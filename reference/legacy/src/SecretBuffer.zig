const builtin = @import("builtin");
const std = @import("std");
const ascii = std.ascii;
const heap = std.heap;
const mem = std.mem;
const posix = std.posix;
const unicode = std.unicode;

const Self = @This();

buffer: []align(mem.page_size) u8,
fba: heap.FixedBufferAllocator,
str: std.ArrayListUnmanaged(u8),
len: usize,

extern fn mlock(addr: *const anyopaque, len: usize) c_int;

pub fn init(self: *Self, alloc: mem.Allocator) !void {
    self.buffer = try alloc.alignedAlloc(u8, mem.page_size, 1024);
    self.fba = heap.FixedBufferAllocator.init(self.buffer);
    self.str = .{};
    self.len = 0;

    // Calling mlock(3) prevents the memory page we use for the password buffer
    // to be swapped.
    {
        var attempts: usize = 0;
        while (attempts < 10) : (attempts += 1) {
            const res = mlock(self.buffer.ptr, self.buffer.len);
            switch (posix.errno(res)) {
                .SUCCESS => break,
                .AGAIN => continue,
                else => return error.UnexpectedError,
            }
        } else {
            return error.MlockFailedTooOften;
        }
    }

    // Prevent this page from showing up in code dumps.
    if (builtin.target.os.tag == .linux) {
        var attempts: usize = 0;
        while (attempts < 10) : (attempts += 1) {
            const res = posix.system.madvise(self.buffer.ptr, self.buffer.len, posix.MADV.DONTDUMP);
            switch (posix.errno(res)) {
                .SUCCESS => break,
                .AGAIN => continue,
                else => return error.UnexpectedError,
            }
        } else {
            return error.MadvideFailedTooOften;
        }
    }
}

pub fn deinit(self: *Self, alloc: mem.Allocator) void {
    alloc.free(self.buffer);
    self.str = undefined;
    self.len = undefined;
}

pub fn reset(self: *Self, alloc: mem.Allocator) !void {
    self.deinit(alloc);
    try self.init(alloc);
}

pub fn appendSlice(self: *Self, str: []const u8) !void {
    const fixed = self.fba.allocator();
    const len = try unicode.utf8CountCodepoints(str);
    try self.str.appendSlice(fixed, str);
    self.len += len;
}

pub fn deleteBackwards(self: *Self) void {
    if (self.str.items.len == 0) return;
    var i: usize = self.str.items.len - 1;
    while (i >= 0) : (i -= 1) {
        _ = unicode.utf8ByteSequenceLength(self.str.items[i]) catch continue;
        const fixed = self.fba.allocator();
        self.str.shrinkAndFree(fixed, i);
        self.len -= 1;
        return;
    }
    unreachable;
}

pub fn slice(self: *Self) ?[]const u8 {
    if (self.str.items.len > 0) {
        return self.str.items[0..];
    } else {
        return null;
    }
}

test "SecretBuffer" {
    const testing = std.testing;
    var buf: Self = undefined;
    try buf.init(testing.allocator);
    try testing.expect(buf.slice() == null);
    try buf.appendSlice("hello");
    try std.testing.expectEqualSlices(u8, "hello", buf.slice().?);
    try buf.reset(testing.allocator);
    try buf.appendSlice("1234");
    try std.testing.expectEqualSlices(u8, "1234", buf.slice().?);
    buf.deleteBackwards();
    try std.testing.expectEqualSlices(u8, "123", buf.slice().?);
    buf.deleteBackwards();
    try std.testing.expectEqualSlices(u8, "12", buf.slice().?);
    buf.deleteBackwards();
    try std.testing.expectEqualSlices(u8, "1", buf.slice().?);
    buf.deleteBackwards();
    try testing.expect(buf.slice() == null);
    buf.deleteBackwards();
    buf.deleteBackwards();
    try buf.appendSlice("abc");
    try std.testing.expectEqualSlices(u8, "abc", buf.slice().?);
    try buf.reset(testing.allocator);
    try buf.appendSlice("a" ** 500);
    try std.testing.expectError(error.OutOfMemory, buf.appendSlice("a" ** 1000));
    buf.deinit(testing.allocator);
}
