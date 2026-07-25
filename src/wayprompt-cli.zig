const std = @import("std");
const posix = std.posix;
const mem = std.mem;
const heap = std.heap;
const process = std.process;
const debug = std.debug;
const io = std.io;
const meta = std.meta;

pub const logFn = @import("log.zig").log;
pub const std_options = std.Options{ .logFn = logFn };

const logger = std.log.scoped(.wayprompt);

const Frontend = @import("Frontend.zig");
const SecretBuffer = @import("SecretBuffer.zig");
const Config = @import("Config.zig");

var getpin: bool = false;
var json: bool = false;

var secret: SecretBuffer = undefined;
var config: Config = undefined;
var frontend: Frontend = undefined;
var gpa: heap.GeneralPurposeAllocator(.{}) = .{};
var arena: heap.ArenaAllocator = undefined;

pub fn main() !u8 {
    const alloc = gpa.allocator();
    defer _ = gpa.deinit();

    arena = heap.ArenaAllocator.init(gpa.allocator());
    defer arena.deinit();

    config = .{
        .secbuf = undefined,
        .alloc = alloc,
    };

    parseCmdFlags() catch |err| {
        switch (err) {
            error.DumpedUsage => return 0,
            error.UnknownFlag,
            error.MissingArgument,
            error.RedundantFlag,
            => return 1,
            else => return err,
        }
    };

    if (!getpin) {
        if (config.labels.prompt != null) {
            logger.err("you may not set a prompt when not querying for a password.", .{});
            return 1;
        }

        if (config.labels.title == null and
            config.labels.description == null and
            config.labels.err_message == null)
        {
            logger.err("at least one of title, description or error need to be set when not querying for a password.", .{});
            return 1;
        }
    }

    // Using the arena for parsing the config will use extra memory on duplicate
    // config fields, however it makes cleanup a lot simpler.
    config.parse(arena.allocator()) catch return 1;

    if (getpin) {
        try secret.init(alloc);
        config.secbuf = &secret;
    }
    defer if (getpin) secret.deinit(alloc);

    var fds: [1]posix.pollfd = .{.{
        .fd = try frontend.init(&config),
        .events = posix.POLL.IN,
        .revents = undefined,
    }};
    defer frontend.deinit();

    try frontend.enterMode(if (getpin) .getpin else .message);

    var ret: u8 = undefined;
    while (true) {
        {
            const ev = try frontend.flush();
            if (ev != .none) {
                try handleEvent(ev, &ret);
                break;
            }
        }

        _ = try posix.poll(&fds, -1);

        if (fds[0].revents & posix.POLL.IN != 0) {
            const ev = try frontend.handleEvent();
            if (ev != .none) {
                try handleEvent(ev, &ret);
                break;
            }
        } else {
            try frontend.noEvent();
        }
    }
    _ = try frontend.flush();

    return ret;
}

fn handleEvent(ev: Frontend.Event, ret: *u8) !void {
    switch (ev) {
        .user_ok => {
            try writeOutput("ok", if (getpin) secret.slice() else null);
            ret.* = 0;
        },
        .user_abort => {
            try writeOutput("cancel", null);
            ret.* = 10;
        },
        .user_notok => {
            try writeOutput("not-ok", null);
            ret.* = 20;
        },
        else => unreachable,
    }
}

fn writeOutput(comptime action: []const u8, pin: ?[]const u8) !void {
    const stdout = io.getStdOut();
    var out_buffer = io.bufferedWriter(stdout.writer());
    const writer = out_buffer.writer();

    if (json) {
        try writer.writeAll("{\n");
        try writer.writeAll("    \"user-action\": \"" ++ action ++ "\"");
        if (pin) |p| {
            debug.assert(getpin);
            try writer.print(",\n    \"pin\": \"{s}\"\n", .{p});
        } else if (getpin) {
            try writer.writeAll(",\n    \"pin\": null\n");
        } else {
            try writer.writeAll("\n");
        }
        try writer.writeAll("}\n");
    } else {
        try writer.writeAll("user-action: " ++ action ++ "\n");
        if (pin) |p| {
            debug.assert(getpin);
            try writer.print("pin: {s}\n", .{p});
        } else if (getpin) {
            try writer.writeAll("no pin\n");
        }
    }

    try out_buffer.flush();
}

fn parseCmdFlags() !void {
    const alloc = arena.allocator();

    var it = process.ArgIteratorPosix.init();

    // On POSIX argv is static.
    const argv0 = it.next() orelse "wayprompt";

    while (it.next()) |flag| {
        if (mem.eql(u8, flag, "--title")) {
            try dupeArg(alloc, &it, &config.labels.title, "--title");
        } else if (mem.eql(u8, flag, "--description")) {
            try dupeArg(alloc, &it, &config.labels.description, "--description");
        } else if (mem.eql(u8, flag, "--prompt")) {
            try dupeArg(alloc, &it, &config.labels.prompt, "--prompt");
        } else if (mem.eql(u8, flag, "--error")) {
            try dupeArg(alloc, &it, &config.labels.err_message, "--error");
        } else if (mem.eql(u8, flag, "--wayland-display")) {
            try dupeArg(alloc, &it, &config.wayland_display, "--wayland-display");
        } else if (mem.eql(u8, flag, "--button-ok")) {
            try dupeArg(alloc, &it, &config.labels.ok, "--button-ok");
        } else if (mem.eql(u8, flag, "--button-not-ok")) {
            try dupeArg(alloc, &it, &config.labels.not_ok, "--button-not-ok");
        } else if (mem.eql(u8, flag, "--button-cancel")) {
            try dupeArg(alloc, &it, &config.labels.cancel, "--button-cancel");
        } else if (mem.eql(u8, flag, "--get-pin")) {
            getpin = true;
        } else if (mem.eql(u8, flag, "--json")) {
            json = true;
        } else if (mem.eql(u8, flag, "--help") or mem.eql(u8, flag, "-h")) {
            const stdout = io.getStdOut();
            var out_buffer = io.bufferedWriter(stdout.writer());
            try out_buffer.writer().print(
                \\Usage: {s} [options..]
                \\  --title           <string>   Set the window title
                \\  --description     <string>   Set the description text.
                \\  --prompt          <string>   Set the prompt. Can only be used with '--get-pin'.
                \\  --error           <string>   Set the error message.
                \\  --button-ok       <string>   Display the ok button with the provided Text.
                \\  --button-not-ok   <string>   Display the not-ok button with the provided Text.
                \\  --button-cancel   <string>   Display the cancel button with the provided Text.
                \\  --wayland-display <string>   Set the WAYLAND_DISPLAY to be used.
                \\  --get-pin                    Query for a password.
                \\  --json                       Format output (except error messages) in JSON.
                \\  --help, -h                   Dump help text and exit.
                \\
                \\This is the command line version of wayprompt, offering a simple API
                \\to use it for example in shell scripts. Read wayprompt.1 for more
                \\information, including how to configure wayprompt and available
                \\alternative versions. wayprompt is developed and maintained by
                \\Leon Henrik Plickat <leonhenrik.plickat@stud.uni-goettingen.de>.
                \\
            , .{argv0});
            try out_buffer.flush();
            return error.DumpedUsage;
        } else {
            logger.err("unknown flag: '{s}'", .{flag});
            return error.UnknownFlag;
        }
    }
}

fn dupeArg(
    alloc: mem.Allocator,
    it: *process.ArgIteratorPosix,
    dest: *?[]const u8,
    comptime flag: []const u8,
) !void {
    if (dest.* != null) {
        logger.err("redundant '{s}' flag.", .{flag});
        return error.RedundantFlag;
    }
    dest.* = try alloc.dupe(u8, it.next() orelse {
        logger.err("flag '{s}' needs an argument.", .{flag});
        return error.MissingArgument;
    });
}
