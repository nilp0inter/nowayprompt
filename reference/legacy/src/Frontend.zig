const std = @import("std");
const mem = std.mem;
const posix = std.posix;
const log = std.log.scoped(.frontend);

const pixman = @import("pixman");

const Wayland = @import("Wayland.zig");
const TTY = @import("TTY.zig");
const SecretBuffer = @import("SecretBuffer.zig");
const Config = @import("Config.zig");

const Frontend = @This();

pub const InterfaceMode = enum {
    none,
    getpin,
    message,
};

pub const Event = enum {
    none,
    user_abort,
    user_notok,
    user_ok,
};

const Implementation = union(enum) {
    wayland: Wayland,
    tty: TTY,
};

impl: Implementation = undefined,

pub fn init(self: *Frontend, cfg: *Config) !posix.fd_t {

    // First we try to do a Wayland.
    self.impl = .{ .wayland = .{} };
    return self.impl.wayland.init(cfg) catch |err| {
        if (err == error.NoWaylandDisplay or err == error.ConnectFailed) {
            if (cfg.allow_tty_fallback) {
                log.info("Switching to TTY fallback.", .{});
                self.impl = .{ .tty = .{} };
                return try self.impl.tty.init(cfg);
            }
        }
        return err;
    };
}

pub fn deinit(self: *Frontend) void {
    switch (self.impl) {
        .wayland => |*w| w.deinit(),
        .tty => |*t| t.deinit(),
    }
}

pub fn enterMode(self: *Frontend, mode: InterfaceMode) !void {
    switch (self.impl) {
        .wayland => |*w| try w.enterMode(mode),
        .tty => |*t| try t.enterMode(mode),
    }
}

pub fn flush(self: *Frontend) !Event {
    switch (self.impl) {
        .wayland => |*w| return try w.flush(),
        .tty => return .none,
    }
}

pub fn handleEvent(self: *Frontend) !Event {
    switch (self.impl) {
        .wayland => |*w| return try w.handleEvent(),
        .tty => |*t| return try t.handleEvent(),
    }
}

pub fn noEvent(self: *Frontend) !void {
    switch (self.impl) {
        .wayland => |*w| try w.noEvent(),
        .tty => {},
    }
}
