const builtin = @import("builtin");
const std = @import("std");
const ascii = std.ascii;
const mem = std.mem;
const posix = std.posix;
const os = std.os;
const io = std.io;
const fs = std.fs;
const fmt = std.fmt;
const heap = std.heap;
const debug = std.debug;

const logFn = @import("log.zig").log;

pub const std_options = std.Options{
    .logFn = logFn,
};

const logger = std.log.scoped(.wayprompt);
var use_syslog = &@import("log.zig").use_syslog;

const Frontend = @import("Frontend.zig");
const SecretBuffer = @import("SecretBuffer.zig");
const Config = @import("Config.zig");

var default_ok: ?[]const u8 = null;
var default_cancel: ?[]const u8 = null;
var default_yes: ?[]const u8 = null;
var default_no: ?[]const u8 = null;

var loop: bool = true;
var secret: SecretBuffer = undefined;
var config: Config = undefined;
var frontend: Frontend = undefined;
var gpa: heap.GeneralPurposeAllocator(.{}) = .{};

/// Not quite the same as Frontend.Mode, as message and confirm behabe slightly
/// differently.
const Mode = enum { none, getpin, message, confirm };
var mode: Mode = .none;

pub fn main() !u8 {
    use_syslog.* = true;

    const alloc = gpa.allocator();
    defer _ = gpa.deinit();

    defer {
        if (default_ok) |str| alloc.free(str);
        if (default_cancel) |str| alloc.free(str);
        if (default_yes) |str| alloc.free(str);
        if (default_no) |str| alloc.free(str);
    }

    try secret.init(alloc);
    defer secret.deinit(alloc);

    config = .{
        .secbuf = &secret,
        .alloc = alloc,
        .allow_tty_fallback = true,
    };
    defer config.reset(alloc);

    config.parse(alloc) catch return 1;

    const stdout = io.getStdOut();
    const stdin = io.getStdIn();

    const fds_stdin = 0;
    const fds_frontend = 1;
    var fds: [2]posix.pollfd = undefined;

    fds[fds_stdin] = .{
        .fd = stdin.handle,
        .events = posix.POLL.IN,
        .revents = undefined,
    };
    var stdin_closed: bool = false;

    fds[fds_frontend] = .{
        .fd = try frontend.init(&config),
        .events = posix.POLL.IN,
        .revents = undefined,
    };
    defer frontend.deinit();

    // Assuan messages are limited to 1000 bytes per spec. However,
    // documentation for the pinentry commands states that string can be up to
    // 2048 bytes long (it actually says "characters", but I assume bytes is
    // what they mean). Since realistically we are the ones sending long strings
    // (i.e. passwords with insane lengths), use a small buffer for input and a
    // default-sized (2048) buffer for outgoing messages.
    var in_buffer: [1024]u8 = undefined;
    var out_buffer = io.bufferedWriter(stdout.writer());
    const writer = out_buffer.writer();
    try writer.writeAll("OK wayprompt is pleased to meet you\n");
    try out_buffer.flush();

    while (loop) {
        if (frontend.flush()) |ev| {
            try handleFrontendEvent(out_buffer.writer(), ev);
        } else |err| {
            logger.err("unexpected error: {s}", .{@errorName(err)});
            try writer.writeAll("ERR 83886179 Operation cancelled\n");
        }

        // We don't poll stdin if it has been closed to avoid pointless spinning.
        _ = try posix.poll(if (stdin_closed) fds[1..2] else &fds, -1);

        if (!stdin_closed) {
            if (fds[fds_stdin].revents & posix.POLL.IN != 0) {
                const read = try stdin.read(&in_buffer);
                if (read == 0) break;

                // The read call may have returned multiple lines at once here, normal command-line
                // buffering does not apply here.
                // TODO: handle partial lines returned by read calls
                var split_iter = std.mem.splitScalar(u8, in_buffer[0..read], '\n');
                while (split_iter.next()) |line| {
                    // The line may be empty in the case of a trailing newline in the read input
                    if (line.len == 0) continue;
                    try parseInput(out_buffer.writer(), line);
                }
            }

            logger.debug("pipe closed.", .{});
            if (fds[fds_stdin].revents & posix.POLL.HUP != 0) {
                stdin_closed = true;
            }
        }

        if (stdin_closed and mode == .none) break;

        if (fds[fds_frontend].revents & posix.POLL.IN != 0) {
            if (frontend.handleEvent()) |ev| {
                try handleFrontendEvent(out_buffer.writer(), ev);
            } else |err| {
                logger.err("unexpected error: {s}", .{@errorName(err)});
                try writer.writeAll("ERR 83886179 Operation cancelled\n");
            }
        } else {
            try frontend.noEvent();
        }

        out_buffer.flush() catch |err| {
            // gpg-agent recently has become very eager to close the pipe after
            // sending "bye", so sending it an "OK" response will fail.
            if (loop == false and err == error.BrokenPipe) break;
            return err;
        };
    }

    return 0;
}

fn handleFrontendEvent(writer: anytype, ev: Frontend.Event) !void {
    debug.assert((mode == .none and ev == .none) or mode != .none);
    switch (ev) {
        .none => return,
        .user_abort => try writer.writeAll("ERR 83886179 Operation cancelled\n"),
        .user_notok => try writer.writeAll("ERR 83886194 not confirmed\n"),
        .user_ok => {
            if (mode == .getpin) {
                // Technically there is a difference between pressing enter on
                // an empty prompt and the user aborting. However, gpg-agent
                // apparently treats both equally. We do it properly of course,
                // because we're pedantic.
                if (secret.slice()) |s| {
                    try writer.print("D {s}\nEND\nOK\n", .{s});
                } else {
                    try writer.writeAll("OK\n");
                }
            } else {
                try writer.writeAll("OK\n");
            }
        },
    }

    // The errormessage must automatically reset after every GETPIN or CONFIRM action.
    if (mode == .getpin or mode == .confirm) {
        if (config.labels.err_message) |e| {
            const alloc = gpa.allocator();
            alloc.free(e);
            config.labels.err_message = null;
        }
    }

    mode = .none;

    const alloc = gpa.allocator();
    try secret.reset(alloc);
}

fn getpin() !void {
    debug.assert(mode == .none);

    // If we don't have buttons defined, use the default ones. Note: This
    // transfers ownership of the string. This means they will be freed when
    // context is deinit'd.
    if (config.labels.ok == null and default_ok != null) {
        config.labels.ok = default_ok.?;
        default_ok = null;
    }
    if (config.labels.cancel == null and default_cancel != null) {
        config.labels.cancel = default_cancel.?;
        default_cancel = null;
    }

    mode = .getpin;
    try frontend.enterMode(.getpin);
}

fn message(writer: anytype) !void {
    debug.assert(mode == .none);

    const labels = config.labels;
    if (labels.title == null and
        labels.description == null and
        labels.err_message == null)
    {
        try writer.writeAll("OK\n");
        return;
    }

    mode = .message;
    try frontend.enterMode(.message);
}

fn confirm() !void {
    debug.assert(mode == .none);

    // If we don't have buttons defined, use the default ones. Note: This
    // transfers ownership of the string. This means they will be freed when
    // context is deinit'd.
    if (config.labels.ok == null and default_yes != null) {
        config.labels.ok = default_yes.?;
        default_yes = null;
    }
    if (config.labels.cancel == null and default_no != null) {
        config.labels.cancel = default_no.?;
        default_no = null;
    }

    mode = .message;
    try frontend.enterMode(.message);
}

fn parseInput(writer: io.BufferedWriter(4096, fs.File.Writer).Writer, line: []const u8) !void {
    // <rant>
    //   The protocol spoken between pinentry and the gpg-agent is a mess. It
    //   uses assuan as the wire protocol (who came up with that name?) which
    //   itself seems reasonable enough for something going through UNIX pipes,
    //   but for pinentry it got extended with additional commands. Nothing is
    //   consistent: Both direct requests as well as arguments to the OPTION
    //   request are used to set options. A server implementing this protocol
    //   is apparently not allowed to say it does not implement optional (!)
    //   requests, no it has to accept them even if it effectively just ignores
    //   them, despite there actually being a "no this is not implemented"
    //   response. Speaking of responses, there are just OK and errors. What?
    //   And errors use the gpg error format, which is ... weird and unwieldy.
    //   And when it comes to data responses, the pinentry program shipped with
    //   gpg just sends the "D string" events followed by OK. This actually goes
    //   against the assuan spec, which states that after the D event you have
    //   to send an END event before sending OK. And funnily enough, we actually
    //   have to do that, as the gpg-agent otherwise aborts. But apparently
    //   the default pinentry program gets away with it somehow? And the
    //   pinentry protocol documentation also says nothing about END? What?
    //   This protocol has clearly suffered severely from having only a single
    //   widely used implementation.
    // </rant>

    // We are not supposed to get any messages (other than password strength
    // indicator messages) while displaying a prompt.
    // TODO or are we?
    if (mode != .none) return;

    const alloc = gpa.allocator();
    var it = mem.tokenize(u8, line, &ascii.whitespace);
    const command = it.next() orelse return;
    if (ascii.eqlIgnoreCase(command, "settitle")) {
        try setString(writer, "title", line["settitle".len..]);
    } else if (ascii.eqlIgnoreCase(command, "setprompt")) {
        try setString(writer, "prompt", line["setprompt".len..]);
    } else if (ascii.eqlIgnoreCase(command, "setdesc")) {
        try setString(writer, "description", line["setdesc".len..]);
    } else if (ascii.eqlIgnoreCase(command, "seterror")) {
        try setString(writer, "err_message", line["seterror".len..]);
    } else if (ascii.eqlIgnoreCase(command, "setok")) {
        try setString(writer, "ok", line["setok ".len..]);
    } else if (ascii.eqlIgnoreCase(command, "setnotok")) {
        try setString(writer, "not_ok", line["setnotok ".len..]);
    } else if (ascii.eqlIgnoreCase(command, "setcancel")) {
        try setString(writer, "cancel", line["setcancel ".len..]);
    } else if (ascii.eqlIgnoreCase(command, "getpin")) {
        // TODO it's possible that the gpg-apgent requests us to ask for the
        //      pin twice (f.e. when the user creates a new one, I imagine).
        //      This needs support in the UI.
        try getpin();
    } else if (ascii.eqlIgnoreCase(command, "confirm")) {
        // TODO this can optionally have the "--one-button" option, in which
        //      case it effectively functions like MESSAGE.
        try confirm();
    } else if (ascii.eqlIgnoreCase(command, "message")) {
        try message(writer);
    } else if (ascii.eqlIgnoreCase(command, "getinfo")) {
        if (it.next()) |info| {
            if (ascii.eqlIgnoreCase(info, "flavor")) {
                try writer.writeAll("D wayprompt\nEND\n");
            } else if (ascii.eqlIgnoreCase(info, "version")) {
                try writer.writeAll("D 0.0.0\nEND\n");
            } else if (ascii.eqlIgnoreCase(info, "pid")) {
                if (builtin.os.tag == .linux) {
                    try writer.print("D {}\n", .{os.linux.getpid()});
                }
                // TODO Get pid on other systems. Do other systems even use GPG
                //      and pinentry programs?
            }
        }
        try writer.writeAll("OK\n");
    } else if (ascii.eqlIgnoreCase(command, "bye")) {
        loop = false;
        try writer.writeAll("OK\n");
    } else if (ascii.eqlIgnoreCase(command, "option")) {
        if (it.next()) |option| {
            if (getOption("putenv=WAYLAND_DISPLAY=", option, line)) |o| {
                if (config.wayland_display) |w| {
                    alloc.free(w);
                    config.wayland_display = null;
                }
                config.wayland_display = try alloc.dupeZ(u8, o);
            } else if (getOption("ttyname=", option, line)) |o| {
                if (config.tty_name) |w| {
                    alloc.free(w);
                    config.tty_name = null;
                }
                config.tty_name = try alloc.dupeZ(u8, o);
            } else if (getOption("default-ok=", option, line)) |o| {
                if (default_ok) |w| {
                    alloc.free(w);
                    default_ok = null;
                }
                default_ok = try pinentryDupe(o, true);
            } else if (getOption("default-cancel=", option, line)) |o| {
                if (default_cancel) |w| {
                    alloc.free(w);
                    default_cancel = null;
                }
                default_cancel = try pinentryDupe(o, true);
            } else if (getOption("default-yes=", option, line)) |o| {
                if (default_yes) |w| {
                    alloc.free(w);
                    default_yes = null;
                }
                default_yes = try pinentryDupe(o, true);
            } else if (getOption("default-no=", option, line)) |o| {
                if (default_no) |w| {
                    alloc.free(w);
                    default_no = null;
                }
                default_no = try pinentryDupe(o, true);
            }
        }
        // Most options are internationalisation for features we don't offer.
        // Unfortunately we have to pretend to accept them. If we ever decide
        // to actually check the validity of the options, the error message for
        // unknown ones would be: "ERR 83886254 Unknown option".
        try writer.writeAll("OK\n");
    } else if (ascii.eqlIgnoreCase(command, "reset")) {
        config.reset(alloc);
        try writer.writeAll("OK\n");
    } else if (ascii.eqlIgnoreCase(command, "setkeyinfo")) {
        // This request sets a key identifier to be used for key-caching
        // mechanism (like for example the keyring daemons employed by some
        // desktop environments). We do not provide this feature (yet? would be
        // neat to have some himitsu integration here), however we unfortunately
        // can not simply respond with "ERR 536870981" (used for unimplemented
        // requests) because the gpg-agent will then abort. So let's just
        // pretend we accept this value and silently ignore it.
        try writer.writeAll("OK\n");
    } else if (ascii.eqlIgnoreCase(command, "cancel") or
        ascii.eqlIgnoreCase(command, "setgenpin") or // Undocumented, but present in "default" pinentry.
        ascii.eqlIgnoreCase(command, "setgenpin_tt") or // Undocumented, but present in "default" pinentry.
        ascii.eqlIgnoreCase(command, "settimeout") or // TODO timeout before aborting the prompt and returning an error.
        ascii.eqlIgnoreCase(command, "end") or
        ascii.eqlIgnoreCase(command, "quit") or // Specified as reserved for future use cases.
        ascii.eqlIgnoreCase(command, "auth") or // Specified as reserved for future use cases.
        ascii.eqlIgnoreCase(command, "cancel") or // Specified as reserved for future use cases.
        ascii.eqlIgnoreCase(command, "clearpassphrase") or // Undocumented, but present in "default" pinentry.
        ascii.eqlIgnoreCase(command, "setrepeat") or // TODO prompt twice for the password, compare them and only accept when equal.
        ascii.eqlIgnoreCase(command, "setrepeaterror") or
        // A qualitybar is technically easy to implement: The argument to the
        // command is the text next to the bar, which we'd probably ignore.
        // If set, we can send "INQUIRE QUALITY <pin>" after every keypress and
        // the client will respond with "<integer>\nEND\n". Would require doing
        // so in the wayland event loop though.
        ascii.eqlIgnoreCase(command, "setqualitybar") or
        ascii.eqlIgnoreCase(command, "setqualitybar_tt"))
    {
        try writer.writeAll("ERR 536870981 Not implemented\n");
    } else if (ascii.eqlIgnoreCase(command, "nop")) {
        try writer.writeAll("OK\n");
    } else if (ascii.eqlIgnoreCase(command, "help")) {
        try writer.writeAll(
            \\# NOP
            \\# SETTITLE
            \\# SETPROMPT
            \\# SETDESC
            \\# SETERROR
            \\# GETPIN
            \\# BYE
            \\# OPTION
            \\# RESET
            \\OK
            \\
        );
    } else {
        try writer.writeAll("ERR 536871187 Unknown IPC command\n");
    }
}

// XXX
fn dumpPin(writer: anytype, pin: ?[]const u8) !void {
    const alloc = gpa.allocator();
    if (pin) |p| {
        defer alloc.free(p);
        try writer.print("D {s}\nEND\nOK\n", .{p});
    } else {
        try writer.writeAll("OK\n");
    }
}

fn getOption(comptime opt: []const u8, arg: []const u8, line: []const u8) ?[]const u8 {
    if (mem.startsWith(u8, arg, opt)) {
        return line["option ".len + opt.len ..];
    }
    return null;
}

fn setString(writer: anytype, comptime name: []const u8, value: []const u8) !void {
    const alloc = gpa.allocator();
    if (@hasField(Config, name)) {
        if (@field(config, name)) |f| {
            @field(config, name) = null;
            alloc.free(f);
        }
        @field(config, name) = try pinentryDupe(value, false);
    } else if (@hasField(@TypeOf(config.labels), name)) {
        if (@field(config.labels, name)) |f| {
            @field(config.labels, name) = null;
            alloc.free(f);
        }
        @field(config.labels, name) = try pinentryDupe(value, false);
    } else {
        @compileError("Field does not exist: " ++ name);
    }
    try writer.writeAll("OK\n");
}

/// Some characters are escaped in assuan messages.
fn pinentryDupe(str: []const u8, button: bool) ![]const u8 {
    const alloc = gpa.allocator();

    var len: usize = str.len;
    for (str) |ch| {
        if (ch == '%') len -= 2;
        if (ch == '_' and button) len -= 1;
    }

    const dupe = try alloc.alloc(u8, len);
    errdefer alloc.free(dupe);

    var i: usize = 0;
    var j: usize = 0;
    while (i < str.len and j < dupe.len) {
        if (str[i] == '%') {
            if (str.len < i + 3) return error.BadInput;
            dupe[j] = try fmt.parseInt(u8, str[i + 1 .. i + 3], 16);
            i += 3;
            j += 1;
        } else if (str[i] == '_' and button) {
            i += 1;
        } else {
            dupe[j] = str[i];
            i += 1;
            j += 1;
        }
    }

    return dupe;
}
