const std = @import("std");
const logger = std.log;
const io = std.io;
const fmt = std.fmt;
const c = @cImport({
    @cInclude("syslog.h");
});

pub var use_syslog: bool = false;

pub fn log(
    comptime level: std.log.Level,
    comptime scope: @TypeOf(.EnumLiteral),
    comptime format: []const u8,
    args: anytype,
) void {
    const level_txt = comptime level.asText();
    const prefix = if (scope == .default) "[wayprompt] " else "[wayprompt, " ++ @tagName(scope) ++ "] ";
    const format_full = prefix ++ level_txt ++ ": " ++ format ++ "\n";

    if (use_syslog) {
        nosuspend syslog(level, format_full, args) catch return;
    } else {
        const stderr = io.getStdErr().writer();
        nosuspend stderr.print(format_full, args) catch return;
    }
}

fn syslog(
    level: logger.Level,
    comptime format: []const u8,
    args: anytype,
) !void {
    const priority = switch (level) {
        .debug => c.LOG_DEBUG,
        .err => c.LOG_ERR,
        .warn => c.LOG_WARNING,
        .info => c.LOG_INFO,
    };
    var buf: [1024]u8 = undefined;
    const str = try fmt.bufPrintZ(&buf, format, args);
    c.syslog(priority, "%s", str.ptr);
}
