const std = @import("std");
const debug = std.debug;
const posix = std.posix;
const io = std.io;
const math = std.math;
const unicode = std.unicode;
const log = std.log.scoped(.backend_tty);

const spoon = @import("spoon");

const SecretBuffer = @import("SecretBuffer.zig");
const Frontend = @import("Frontend.zig");
const Config = @import("Config.zig");

const TTY = @This();

const LineIterator = struct {
    in: ?[]const u8,

    pub fn from(input: []const u8) LineIterator {
        return .{ .in = input };
    }

    pub fn next(self: *LineIterator) ?[]const u8 {
        if (self.in == null) return null;
        if (self.in.?.len == 0) return null;
        if (self.in.?.len == 1 and self.in.?[0] == '\n') return null;
        var i: usize = 0;
        for (self.in.?) |byte| {
            if (byte == '\n') {
                defer self.in = self.in.?[i + 1 ..];
                return self.in.?[0..i];
            }
            i += 1;
        }
        defer self.in = null;
        return self.in.?;
    }
};

term: spoon.Term = undefined,
config: *Config = undefined,
mode: Frontend.InterfaceMode = .none,

pub fn init(self: *TTY, cfg: *Config) !posix.fd_t {
    self.config = cfg;

    // Only try to fall back to TTY mode when a TTY is set.
    if (cfg.tty_name) |name| {
        log.debug("Trying to use TTY: '{s}'", .{name});
        try self.term.init(.{ .tty_name = name });
    } else {
        return error.NoTTYNameSet;
    }

    return self.term.tty.?;
}

pub fn deinit(self: *TTY) void {
    self.term.cook() catch |err| {
        log.err("failed to cook terminal: {s}", .{@errorName(err)});
    };
}

pub fn enterMode(self: *TTY, mode: Frontend.InterfaceMode) !void {
    debug.assert(self.mode != mode);
    if (mode == .none) {
        try self.term.cook();
    } else {
        debug.assert(self.mode == .none);
        try self.term.uncook(.{});
        try self.term.fetchSize();

        if (self.config.labels.title) |t| {
            try self.term.setWindowTitle("wayprompt TTY fallback: {s}", .{t});
        } else {
            try self.term.setWindowTitle("wayprompt TTY fallback", .{});
        }
        try self.render();
    }
}

pub fn handleEvent(self: *TTY) !Frontend.Event {
    var ret: Frontend.Event = .none;
    var buf: [32]u8 = undefined;
    const read = try self.term.readInput(&buf);
    var it = spoon.inputParser(buf[0..read]);
    while (it.next()) |in| {
        if (in.eqlDescription("enter")) {
            ret = .user_ok;
            break;
        } else if (in.eqlDescription("escape")) {
            ret = .user_abort;
            break;
        } else if (in.eqlDescription("C-c")) {
            if (self.config.labels.not_ok == null) {
                ret = .user_abort;
            } else {
                ret = .user_notok;
            }
            break;
        } else if (in.eqlDescription("C-u") or in.eqlDescription("C-w") or in.eqlDescription("C-backspace")) {
            if (self.mode == .getpin) {
                try self.config.secbuf.reset(self.config.alloc);
                try self.render();
            }
        } else if (in.eqlDescription("backspace")) {
            if (self.mode == .getpin) {
                self.config.secbuf.deleteBackwards();
                try self.render();
            }
        } else if (self.mode == .getpin and in.content == .codepoint) {
            if (in.mod_alt or in.mod_ctrl or in.mod_super) continue;
            const cp = in.content.codepoint;

            // We can safely reuse the buffer here.
            const len = unicode.utf8Encode(cp, &buf) catch |err| {
                log.err("Failed to encode unicode codepoint: {s}", .{@errorName(err)});
                continue;
            };
            self.config.secbuf.appendSlice(buf[0..len]) catch |err| {
                log.err("Failed to append slice to SecretBuffer: {s}", .{@errorName(err)});
            };

            try self.render();
        }
    }
    return ret;
}

// TODO listen to SIGWINCH
fn render(self: *TTY) !void {
    var rc = try self.term.getRenderContext();
    defer rc.done() catch {};
    try rc.clear();

    if (self.term.width < 5 or self.term.height < 5) {
        try rc.setAttribute(.{ .fg = .red, .bold = true });
        try rc.writeAllWrapping("Terminal too small!");
        return;
    }

    var line: usize = 0;

    const labels = self.config.labels;
    if (labels.title) |t| try self.renderContent(&rc, t, .{ .bg = .green, .bold = true, .fg = .black }, &line);
    if (labels.description) |d| try self.renderContent(&rc, d, .{}, &line);
    if (labels.prompt) |p| try self.renderContent(&rc, p, .{ .bold = true }, &line);

    if (self.mode == .getpin) {
        try rc.setAttribute(.{ .bold = true });
        try rc.moveCursorTo(line, 0);
        var rpw = rc.restrictedPaddingWriter(self.term.width);
        const writer = rpw.writer();
        try writer.writeAll(" > ");
        const pin_square_amount = self.config.wayland_ui.pin_square_amount;
        const len = self.config.secbuf.len;
        try writer.writeByteNTimes('*', @min(pin_square_amount, len));
        try writer.writeByteNTimes('_', pin_square_amount -| len);
        try rpw.finish();
        line += 2;
    }

    if (labels.err_message) |e| try self.renderContent(&rc, e, .{ .bold = true, .fg = .red }, &line);

    if (labels.ok) |o| try self.renderButton(&rc, "enter", o, &line);
    if (labels.not_ok) |n| try self.renderButton(&rc, "C-c", n, &line);
    if (labels.cancel) |c| try self.renderButton(&rc, "escape", c, &line);
}

fn renderContent(self: *TTY, rc: *spoon.Term.RenderContext, str: []const u8, attr: spoon.Attribute, line: *usize) !void {
    try rc.setAttribute(attr);
    var it = LineIterator.from(str);
    while (it.next()) |l| {
        if (line.* >= self.term.height) return;
        try rc.moveCursorTo(line.*, 0);
        var rpw = rc.restrictedPaddingWriter(self.term.width);
        const writer = rpw.writer();
        try writer.writeByte(' ');
        try writer.writeAll(l);
        if (attr.bg != .none) {
            try rpw.pad();
        } else {
            try rpw.finish();
        }
        line.* += 1;
    }
    line.* += 1;
}

fn renderButton(self: *TTY, rc: *spoon.Term.RenderContext, comptime button: []const u8, str: []const u8, line: *usize) !void {
    const first = line.*;
    try rc.setAttribute(.{});
    try rc.moveCursorTo(line.*, 0);
    var it = LineIterator.from(str);
    while (it.next()) |l| {
        if (line.* >= self.term.height) return;
        try rc.moveCursorTo(line.*, 0);
        var rpw = rc.restrictedPaddingWriter(self.term.width);
        const writer = rpw.writer();
        try writer.writeByte(' ');
        if (line.* == first) {
            try writer.writeAll(button);
            try writer.writeAll(": ");
        } else {
            try writer.writeByteNTimes(' ', button.len + ": ".len);
        }
        try writer.writeAll(l);
        try rpw.finish();
        line.* += 1;
    }
}
