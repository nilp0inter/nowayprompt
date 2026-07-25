const builtin = @import("builtin");
const std = @import("std");
const ascii = std.ascii;
const posix = std.posix;
const mem = std.mem;
const math = std.math;
const unicode = std.unicode;
const debug = std.debug;
const log = std.log.scoped(.backend_wayland);

const pixman = @import("pixman");
const fcft = @import("fcft");
const xkb = @import("xkbcommon");
const wayland = @import("wayland");
const wl = wayland.client.wl;
const wp = wayland.client.wp;
const zwlr = wayland.client.zwlr;

const Frontend = @import("Frontend.zig");
const Config = @import("Config.zig");

const Wayland = @This();

const HotSpot = struct {
    const Effect = enum { cancel, ok, notok };

    effect: Effect,
    x: u31,
    y: u31,
    width: u31,
    height: u31,

    pub fn containsPoint(self: HotSpot, x: u31, y: u31) bool {
        return x >= self.x and x <= self.x +| self.width and
            y >= self.y and y <= self.y +| self.height;
    }

    pub fn act(self: HotSpot, w: *Wayland) void {
        switch (self.effect) {
            .cancel => w.abort(error.UserAbort),
            .notok => w.abort(error.UserNotOk),
            .ok => w.abort(error.UserOk),
        }
    }
};

// Copied and adapted from https://git.sr.ht/~novakane/zelbar, same license.
const TextView = struct {
    const Mode = union(enum) {
        text_run: *const fcft.TextRun,
        glyphs: struct {
            glyphs: []*const fcft.Glyph,
            kerns: []c_long,
        },
    };

    mode: Mode,
    font: *fcft.Font,
    width: u31,
    height: u31,

    pub fn new(alloc: mem.Allocator, str: []const u8, font: *fcft.Font) !TextView {
        if (str.len == 0) return error.EmptyString;

        var height: u31 = @intCast(font.height);

        const len = try unicode.utf8CountCodepoints(str);
        const codepoints = try alloc.alloc(u32, len);
        defer alloc.free(codepoints);
        {
            var i: usize = 0;
            var it = (try unicode.Utf8View.init(str)).iterator();
            while (it.nextCodepoint()) |cp| : (i += 1) {
                if (cp == '\n') height += @as(u31, @intCast(font.height));
                codepoints[i] = cp;
            }
        }

        if ((fcft.capabilities() & fcft.Capabilities.text_run_shaping) != 0) {
            const text_run = try font.rasterizeTextRunUtf32(codepoints, .default);
            var i: usize = 0;
            var width: u31 = 0;
            var max_width: u31 = 0;
            while (i < text_run.count) : (i += 1) {
                if (text_run.glyphs[i].cp == '\n') {
                    if (width > max_width) {
                        max_width = width;
                    }
                    width = 0;
                } else {
                    width += @as(u31, @intCast(text_run.glyphs[i].advance.x));
                }
            }
            if (width > max_width) {
                max_width = width;
            }

            return TextView{
                .mode = .{ .text_run = text_run },
                .font = font,
                .width = max_width,
                .height = height,
            };
        } else {
            const glyphs = try alloc.alloc(*const fcft.Glyph, codepoints.len);
            errdefer alloc.free(glyphs);
            const kerns = try alloc.alloc(c_long, codepoints.len);
            errdefer alloc.free(kerns);

            var i: usize = 0;
            var width: u31 = 0;
            var max_width: u31 = 0;
            while (i < codepoints.len) : (i += 1) {
                glyphs[i] = try font.rasterizeCharUtf32(codepoints[i], .default);
                kerns[i] = 0;
                if (i > 0) {
                    var x_kern: c_long = 0;
                    if (font.kerning(codepoints[i - 1], codepoints[i], &x_kern, null)) kerns[i] = x_kern;
                }
                if (glyphs[i].cp == '\n') {
                    if (width > max_width) {
                        max_width = width;
                    }
                    width = 0;
                } else {
                    width += @as(u31, @intCast(kerns[i] + glyphs[i].advance.x));
                }
            }
            if (width > max_width) {
                max_width = width;
            }

            return TextView{
                .mode = .{ .glyphs = .{
                    .glyphs = glyphs,
                    .kerns = kerns,
                } },
                .font = font,
                .width = max_width,
                .height = height,
            };
        }
    }

    pub fn deinit(self: *const TextView, alloc: mem.Allocator) void {
        switch (self.*.mode) {
            .text_run => self.mode.text_run.destroy(),
            .glyphs => {
                alloc.free(self.mode.glyphs.glyphs);
                alloc.free(self.mode.glyphs.kerns);
            },
        }
    }

    pub fn draw(self: *const TextView, image: *pixman.Image, colour: *const pixman.Color, x: u31, y: u31, vertical_padding: u31) !u31 {
        const glyphs = switch (self.mode) {
            .text_run => self.mode.text_run.glyphs[0..self.mode.text_run.count],
            .glyphs => self.mode.glyphs.glyphs,
        };

        var X: u31 = x;
        var Y: u31 = y;
        var i: usize = 0;
        while (i < glyphs.len) : (i += 1) {
            if (self.mode == .glyphs) X += @as(u31, @intCast(self.mode.glyphs.kerns[i]));

            if (glyphs[i].cp == '\n') {
                X = x;
                Y += @as(u31, @intCast(self.font.height));
                continue;
            }

            const solcol = pixman.Image.createSolidFill(colour).?;
            defer _ = solcol.unref();

            switch (pixman.Image.getFormat(glyphs[i].pix)) {
                // Pre-rendered Image.
                .a8r8g8b8 => pixman.Image.composite32(
                    .atop,
                    glyphs[i].pix,
                    null,
                    image,
                    0,
                    0,
                    0,
                    0,
                    X + @as(u31, @intCast(glyphs[i].x)),
                    Y - @as(i32, @intCast(glyphs[i].y)) + self.font.ascent,
                    glyphs[i].width,
                    glyphs[i].height,
                ),

                // Alpha mask (i.e. regular character).
                else => {
                    pixman.Image.composite32(
                        .atop,
                        solcol,
                        glyphs[i].pix,
                        image,
                        0,
                        0,
                        0,
                        0,
                        X + @as(i32, @intCast(glyphs[i].x)),
                        Y - @as(i32, @intCast(glyphs[i].y)) + self.font.ascent,
                        glyphs[i].width,
                        glyphs[i].height,
                    );
                },
            }

            X += @as(u31, @intCast(glyphs[i].advance.x));
        }

        return self.height + vertical_padding;
    }
};

const Seat = struct {
    const TouchPoint = struct {
        id: i32,
        hotspot: *HotSpot,
    };

    w: *Wayland,

    wl_seat: *wl.Seat,

    // Keyboard related objects.
    wl_keyboard: ?*wl.Keyboard = null,
    xkb_state: ?*xkb.State = null,

    // Pointer related objects.
    wl_pointer: ?*wl.Pointer = null,
    cursor_shape_device: ?*wp.CursorShapeDeviceV1 = null,
    pointer_x: u31 = 0,
    pointer_y: u31 = 0,
    cursor_shape: ?wp.CursorShapeDeviceV1.Shape = null,
    cursor_theme: ?*wl.CursorTheme = null,
    cursor_surface: ?*wl.Surface = null,
    last_enter_serial: u32 = undefined,
    press_hotspot: ?*HotSpot = null,

    // Touch related objects.
    wl_touch: ?*wl.Touch = null,
    touchpoints: std.TailQueue(TouchPoint) = .{},

    pub fn init(self: *Seat, w: *Wayland, wl_seat: *wl.Seat) !void {
        self.* = .{ .w = w, .wl_seat = wl_seat };
        self.wl_seat.setListener(*Seat, seatListener, self);
    }

    pub fn deinit(self: *Seat) void {
        self.releaseKeyboard();
        self.releasePointer();
        self.releaseTouch();
        self.wl_seat.destroy();
    }

    fn seatListener(_: *wl.Seat, event: wl.Seat.Event, self: *Seat) void {
        switch (event) {
            .capabilities => |ev| {
                if (ev.capabilities.keyboard) {
                    self.bindKeyboard() catch {};
                } else {
                    self.releaseKeyboard();
                }

                if (ev.capabilities.pointer) {
                    self.bindPointer() catch {};
                } else {
                    self.releasePointer();
                }

                if (ev.capabilities.touch) {
                    self.bindTouch() catch {};
                } else {
                    self.releaseTouch();
                }
            },
            .name => {}, // Do I look like I care?
        }
    }

    fn bindPointer(self: *Seat) !void {
        if (self.wl_pointer != null) return;
        debug.assert(self.cursor_shape == null);
        self.wl_pointer = try self.wl_seat.getPointer();
        self.wl_pointer.?.setListener(*Seat, pointerListener, self);
        if (self.w.cursor_shape_manager) |csm| {
            self.cursor_shape_device = try csm.getPointer(self.wl_pointer.?);
        }
    }

    fn releasePointer(self: *Seat) void {
        self.cursor_shape = null;
        self.press_hotspot = null;
        if (self.cursor_theme) |t| {
            t.destroy();
            self.cursor_theme = null;
        }
        if (self.cursor_surface) |s| {
            s.destroy();
            self.cursor_surface = null;
        }
        if (self.wl_pointer) |p| {
            p.release();
            self.wl_pointer = null;
        }
        if (self.cursor_shape_device) |csd| {
            csd.destroy();
            self.cursor_shape_device = null;
        }
    }

    fn pointerListener(_: *wl.Pointer, event: wl.Pointer.Event, self: *Seat) void {
        // It is possible that the server sends us more move events before we
        // inform it of having closed the surface.
        if (self.w.surface == null) return;

        switch (event) {
            .enter => |ev| self.updatePointer(ev.surface_x, ev.surface_y, ev.serial),
            .leave => self.cursor_shape = null,
            .motion => |ev| self.updatePointer(ev.surface_x, ev.surface_y, null),
            .button => |ev| {
                // Only activating a button on release is the better UX, IMO.
                switch (ev.state) {
                    .pressed => self.press_hotspot = self.w.surface.?.hotspotFromPoint(self.pointer_x, self.pointer_y),
                    .released => {
                        if (self.press_hotspot == null) return;
                        if (self.w.surface.?.hotspotFromPoint(self.pointer_x, self.pointer_y)) |hs| {
                            if (hs == self.press_hotspot.?) {
                                hs.act(self.w);
                            }
                        }
                        self.press_hotspot = null;
                    },
                    else => {},
                }
            },
            else => {},
        }
    }

    fn updatePointer(self: *Seat, x: wl.Fixed, y: wl.Fixed, serial: ?u32) void {
        const X = x.toInt();
        self.pointer_x = if (X > 0) @as(u31, @intCast(X)) else 0;

        const Y = y.toInt();
        self.pointer_y = if (Y > 0) @as(u31, @intCast(Y)) else 0;

        if (serial) |s| self.last_enter_serial = s;

        // Sanity check.
        debug.assert(self.wl_pointer != null);

        // Cursor errors shall not be fatal. It's fairly expectable for
        // something to go wrong there and it's not exactly vital to our
        // operation here, so we can roll without setting the cursor.
        if (self.w.surface.?.hotspotFromPoint(self.pointer_x, self.pointer_y) != null) {
            self.setCursor(.pointer) catch {};
        } else {
            self.setCursor(.default) catch {};
        }
    }

    fn setCursor(self: *Seat, shape: wp.CursorShapeDeviceV1.Shape) !void {
        if (self.cursor_shape) |cs| {
            if (cs == shape) return;
        }
        self.cursor_shape = shape;

        log.debug("Setting cursor: {s}", .{@tagName(shape)});

        // If the Wayland server supports the CursorShapeV1 protocol, use that
        // to set the cursor. Otherwise we need to go through wl-cursor.
        if (self.cursor_shape_device) |csd| {
            csd.setShape(self.last_enter_serial, shape);
        } else {
            try self.setCursorSurface(shape);
        }
    }

    fn setCursorSurface(self: *Seat, shape: wp.CursorShapeDeviceV1.Shape) !void {
        const name = @tagName(shape);

        const scale = 1; // TODO
        const cursor_size = 24 * scale;

        if (self.cursor_theme == null) {
            self.cursor_theme = try wl.CursorTheme.load(null, cursor_size, self.w.shm.?);
        }
        errdefer {
            self.cursor_theme.?.destroy();
            self.cursor_theme = null;
        }

        // These just point back to the CursorTheme, no need to keep them.
        const wl_cursor = self.cursor_theme.?.getCursor(name) orelse return error.NoCursor;
        const cursor_image = wl_cursor.images[0]; // TODO Is this nullable? Not in the bindings, but they may be wrong.
        const wl_buffer = try cursor_image.getBuffer();

        if (self.cursor_surface == null) {
            self.cursor_surface = try self.w.compositor.?.createSurface();
        }
        errdefer {
            self.cursor_surface.?.destroy();
            self.cursor_surface = null;
        }

        self.cursor_surface.?.setBufferScale(scale);
        self.cursor_surface.?.attach(wl_buffer, 0, 0);
        self.cursor_surface.?.damageBuffer(0, 0, math.maxInt(i31), math.maxInt(u31));
        self.cursor_surface.?.commit();

        self.wl_pointer.?.setCursor(
            self.last_enter_serial,
            self.cursor_surface.?,
            @intCast(@divFloor(cursor_image.hotspot_x, scale)),
            @intCast(@divFloor(cursor_image.hotspot_y, scale)),
        );
    }

    fn bindKeyboard(self: *Seat) !void {
        if (self.wl_keyboard != null) return;
        self.wl_keyboard = try self.wl_seat.getKeyboard();
        self.wl_keyboard.?.setListener(*Seat, keyboardListener, self);
        // TODO XKBcommon things (see snayk)
    }

    fn releaseKeyboard(self: *Seat) void {
        if (self.wl_keyboard) |kb| {
            kb.release();
            self.wl_keyboard = null;
            // TODO XKBcommon things (see snayk)
        }
        if (self.xkb_state) |xs| {
            xs.unref();
            self.xkb_state = null;
        }
    }

    fn keyboardListener(_: *wl.Keyboard, event: wl.Keyboard.Event, self: *Seat) void {
        switch (event) {
            .keymap => |ev| {
                defer posix.close(ev.fd);
                if (ev.format != .xkb_v1) {
                    self.w.abort(error.UnsupportedKeyboardLayoutFormat);
                    return;
                }
                const keymap_str = posix.mmap(null, ev.size, posix.PROT.READ, posix.MAP{ .TYPE = std.os.linux.MAP_TYPE.PRIVATE }, ev.fd, 0) catch {
                    self.w.abort(error.OutOfMemory);
                    return;
                };
                defer posix.munmap(keymap_str);
                const keymap = xkb.Keymap.newFromBuffer(
                    self.w.xkb_context.?,
                    keymap_str.ptr,
                    keymap_str.len - 1,
                    .text_v1,
                    .no_flags,
                ) orelse {
                    self.w.abort(error.OutOfMemory);
                    return;
                };
                defer keymap.unref();
                const state = xkb.State.new(keymap) orelse {
                    self.w.abort(error.OutOfMemory);
                    return;
                };
                defer state.unref();
                if (self.xkb_state) |xs| xs.unref();
                self.xkb_state = state.ref();
            },
            .modifiers => |ev| {
                if (self.xkb_state) |xs| {
                    _ = xs.updateMask(ev.mods_depressed, ev.mods_latched, ev.mods_locked, 0, 0, ev.group);
                }
            },
            .key => |ev| {
                if (ev.state != .pressed) return;
                const keycode = ev.key + 8;
                const keysym = self.xkb_state.?.keyGetOneSym(keycode);
                if (keysym == .NoSymbol) return;

                if (self.xkb_state.?.modNameIsActive(
                    xkb.names.mod.ctrl,
                    @as(xkb.State.Component, @enumFromInt(xkb.State.Component.mods_effective)),
                ) == 1) {
                    switch (@intFromEnum(keysym)) {
                        xkb.Keysym.BackSpace, xkb.Keysym.u, xkb.Keysym.w => {
                            if (self.w.mode == .getpin) {
                                self.w.config.secbuf.reset(self.w.config.alloc) catch self.w.abort(error.OutOfMemory);
                                self.w.surface.?.render() catch self.w.abort(error.OutOfMemory);
                            }
                        },
                        else => {},
                    }
                } else {
                    switch (@intFromEnum(keysym)) {
                        xkb.Keysym.Return => {
                            self.w.abort(error.UserOk);
                            return;
                        },
                        xkb.Keysym.BackSpace => {
                            if (self.w.mode == .getpin) {
                                self.w.config.secbuf.deleteBackwards();
                                self.w.surface.?.render() catch self.w.abort(error.OutOfMemory);
                            }
                            return;
                        },
                        xkb.Keysym.Delete => return,
                        xkb.Keysym.Escape => {
                            self.w.abort(error.UserAbort);
                            return;
                        },
                        else => {},
                    }

                    if (self.w.mode != .getpin) return;

                    var buffer: [16]u8 = undefined;
                    const used = self.xkb_state.?.keyGetUtf8(keycode, &buffer);
                    self.w.config.secbuf.appendSlice(buffer[0..used]) catch {};

                    // We only get keyboard input when a surface exists.
                    self.w.surface.?.render() catch self.w.abort(error.OutOfMemory);
                }
            },
            .enter => {},
            .leave => {},
            .repeat_info => {},
        }
    }

    fn bindTouch(seat: *Seat) !void {
        if (seat.wl_touch != null) return;
        debug.assert(seat.touchpoints.len == 0);
        seat.wl_touch = try seat.wl_seat.getTouch();
        seat.wl_touch.?.setListener(*Seat, touchListener, seat);
    }

    fn releaseTouch(seat: *Seat) void {
        if (seat.wl_touch) |t| {
            t.destroy();
            seat.wl_touch = null;

            var it = seat.touchpoints.first;
            while (it) |node| {
                // We need to get the next node before destroying the current one.
                it = node.next;
                seat.touchpoints.remove(node);
                seat.w.config.alloc.destroy(node);
            }
        }
    }

    fn touchListener(_: *wl.Touch, event: wl.Touch.Event, seat: *Seat) void {
        // It is possible that the server sends us more move events before we
        // inform it of having closed the surface.
        const surface: *Surface = &(seat.w.surface orelse return);
        const alloc = seat.w.config.alloc;

        switch (event) {
            .down => |ev| {
                const x = @as(u24, @intCast(math.clamp(ev.x.toInt(), 0, math.maxInt(i24))));
                const y = @as(u24, @intCast(math.clamp(ev.y.toInt(), 0, math.maxInt(i24))));

                // We can safely ignore a touch-down on a non-hotspot
                // area.
                const hotspot = surface.hotspotFromPoint(x, y) orelse return;

                // Create new touch-point. This is necessary, because
                // we want to activate hotspots on touch-up, which
                // generally is better UX. We have a list of
                // touchpoints instead of a single one so that we can
                // support multi-touch operation.
                const node = alloc.create(std.TailQueue(TouchPoint).Node) catch return;
                node.data = .{
                    .id = ev.id,
                    .hotspot = hotspot,
                };
                seat.touchpoints.append(node);
            },

            .up => |ev| {
                const node = seat.touchPointNodeFromId(ev.id) orelse return;
                defer alloc.destroy(node);
                seat.touchpoints.remove(node);
                node.data.hotspot.act(seat.w);
            },

            .motion => |ev| {
                // If the touchpoint has moved out of the hotspot, destroy it.
                const x = @as(u24, @intCast(math.clamp(ev.x.toInt(), 0, math.maxInt(i24))));
                const y = @as(u24, @intCast(math.clamp(ev.y.toInt(), 0, math.maxInt(i24))));

                const node = seat.touchPointNodeFromId(ev.id) orelse return;
                if (!node.data.hotspot.containsPoint(x, y)) {
                    seat.touchpoints.remove(node);
                    alloc.destroy(node);
                }
            },

            .cancel => {
                // When the cancel event is sent, all touchpoints
                // become invalid.
                var it = seat.touchpoints.first;
                while (it) |node| {
                    // We need to get the next node before destroying the current one.
                    it = node.next;
                    seat.touchpoints.remove(node);
                    alloc.destroy(node);
                }
            },

            // Our use case is simple enough that we don't need to
            // care about frames.
            .frame => {},
            .shape => {},
            .orientation => {},
        }
    }

    fn touchPointNodeFromId(seat: *Seat, id: i32) ?*std.TailQueue(TouchPoint).Node {
        debug.assert(seat.wl_touch != null);
        var it = seat.touchpoints.first;
        while (it) |node| : (it = node.next) {
            if (node.data.id == id) return node;
        }
        return null;
    }
};

/// Pixman images of circle and circle outline, used as masks to draw rounded corners.
const CircleMask = struct {
    outline: ?*pixman.Image = null,
    background: ?*pixman.Image = null,

    outline_data: ?[]u8 = null,
    background_data: ?[]u8 = null,

    pub fn init(self: *CircleMask, alloc: mem.Allocator, radius: u15, border: u31) !void {
        debug.assert(self.outline == null);
        debug.assert(self.outline_data == null);
        debug.assert(self.background == null);
        debug.assert(self.background_data == null);

        const size = radius * 2;

        // I am way to lazy to port this insane C macro to zig code,
        // especially since the format is hardcoded.
        const pixman_format_bpp_of_a8 = 8;
        const stride: u31 = pixman_format_bpp_of_a8 * size;

        self.outline_data = try alloc.alloc(u8, size * stride);
        errdefer {
            alloc.free(self.outline_data.?);
            self.outline_data = null;
        }
        self.outline = pixman.Image.createBits(
            .a8,
            @intCast(size),
            @intCast(size),
            @as([*c]u32, @alignCast(@ptrCast(self.outline_data.?.ptr))),
            @intCast(stride),
        );
        errdefer _ = self.outline.?.unref();

        self.background_data = try alloc.alloc(u8, size * stride);
        errdefer {
            alloc.free(self.background_data.?);
            self.background_data = null;
        }
        self.background = pixman.Image.createBits(
            .a8,
            @intCast(size),
            @intCast(size),
            @as([*c]u32, @alignCast(@ptrCast(self.background_data.?.ptr))),
            @intCast(stride),
        );
        errdefer _ = self.background.?.unref();

        // Clear.
        // TODO XXX is this needed?
        const transparent = pixman.Color{ .red = 0, .green = 0, .blue = 0, .alpha = 0 };
        _ = pixman.Image.fillRectangles(.src, self.outline.?, &transparent, 1, &[1]pixman.Rectangle16{
            .{ .x = 0, .y = 0, .width = size, .height = size },
        });
        _ = pixman.Image.fillRectangles(.src, self.background.?, &transparent, 1, &[1]pixman.Rectangle16{
            .{ .x = 0, .y = 0, .width = size, .height = size },
        });

        const R: f32 = @floatFromInt(radius);
        const b: f32 = @floatFromInt(border);
        const center = @as(f32, @floatFromInt(size)) / 2.0;
        for (0..size) |x| {
            const diff_x: f32 = center - (@as(f32, @floatFromInt(x)) + 0.5);
            for (0..size) |y| {
                const diff_y: f32 = center - (@as(f32, @floatFromInt(y)) + 0.5);
                const distance_to_center = @sqrt(
                    (diff_x * diff_x) + (diff_y * diff_y),
                );

                // Fake anti-aliasing.
                if (distance_to_center < R + 0.5 and distance_to_center > R - (b + 0.3)) {
                    self.outline_data.?[y * stride + x] = 0x44;
                }

                if (distance_to_center < R) {
                    if (distance_to_center > R - b) {
                        self.outline_data.?[y * stride + x] = 0xff;
                    }
                    self.background_data.?[y * stride + x] = 0xff;
                }
            }
        }
    }

    pub fn deinit(self: *CircleMask, alloc: mem.Allocator) void {
        if (self.outline) |o| {
            _ = o.unref();
            self.outline = null;
        }
        if (self.background) |b| {
            _ = b.unref();
            self.background = null;
        }
        if (self.outline_data) |o| {
            alloc.free(o);
            self.outline_data = null;
        }
        if (self.background_data) |b| {
            alloc.free(b);
            self.background_data = null;
        }
    }
};

const Surface = struct {
    w: *Wayland = undefined,

    wl_surface: *wl.Surface = undefined,
    layer_surface: *zwlr.LayerSurfaceV1 = undefined,
    configured: bool = false,
    width: u31 = undefined,
    height: u31 = undefined,

    scale: u31 = 1, // TODO use buffer_scale events

    circle: CircleMask = .{},

    /// Cursor / Touch hotspots, populated on first render.
    hotspots: std.ArrayListUnmanaged(HotSpot) = .{},

    pub fn init(self: *Surface, w: *Wayland) !void {
        log.debug("creating surface.", .{});

        const wl_surface = try w.compositor.?.createSurface();
        errdefer wl_surface.destroy();

        const layer_surface = try w.layer_shell.?.getLayerSurface(wl_surface, null, .overlay, "wayprompt");
        errdefer layer_surface.destroy();

        self.* = .{
            .w = w,
            .wl_surface = wl_surface,
            .layer_surface = layer_surface,
        };
        try self.calculateSize();

        const uiconf = w.config.wayland_ui;
        if (uiconf.corner_radius > 0) {
            const corner_radius = @min(
                uiconf.corner_radius,
                @divFloor(self.width, 2),
                @divFloor(self.height, 2),
            );
            try self.circle.init(self.w.config.alloc, corner_radius, uiconf.border);
        }

        layer_surface.setListener(*Surface, layerSurfaceListener, self);
        layer_surface.setKeyboardInteractivity(.exclusive);
        layer_surface.setSize(self.width, self.height);
        wl_surface.commit();
    }

    fn calculateSize(self: *Surface) !void {
        const uiconf = self.w.config.wayland_ui;
        self.height = uiconf.vertical_padding;
        self.width = uiconf.horizontal_padding;

        if (self.w.mode == .getpin) {
            if (self.w.prompt) |prompt| {
                self.width = @max(prompt.width + 2 * uiconf.horizontal_padding, self.width);
                self.height += prompt.height + uiconf.vertical_padding;
            }

            const square_padding = @divFloor(uiconf.pin_square_size, 2);
            const pinarea_height = uiconf.pin_square_size + 2 * square_padding;
            const pinarea_width = uiconf.pin_square_amount * (uiconf.pin_square_size + square_padding) + square_padding;

            self.height += pinarea_height + uiconf.vertical_padding;
            self.width = @max(self.width, pinarea_width + 2 * uiconf.horizontal_padding);
        }

        if (self.w.title) |title| {
            self.width = @max(title.width + 2 * uiconf.horizontal_padding, self.width);
            self.height += title.height + uiconf.vertical_padding;
        }
        if (self.w.description) |description| {
            self.width = @max(description.width + 2 * uiconf.horizontal_padding, self.width);
            self.height += description.height + uiconf.vertical_padding;
        }
        if (self.w.errmessage) |errmessage| {
            self.width = @max(errmessage.width + 2 * uiconf.horizontal_padding, self.width);
            self.height += errmessage.height + uiconf.vertical_padding;
        }

        {
            var button_amount: u31 = 0;
            var combined_button_length: u31 = 0;
            var max_button_height: u31 = 0;

            if (self.w.ok) |ok| {
                button_amount += 1;
                combined_button_length += ok.width + uiconf.horizontal_padding + 2 * uiconf.button_inner_padding;
                max_button_height = @max(max_button_height, ok.height + 2 * uiconf.button_inner_padding);
            }
            if (self.w.notok) |notok| {
                button_amount += 1;
                combined_button_length += notok.width + uiconf.horizontal_padding + 2 * uiconf.button_inner_padding;
                max_button_height = @max(max_button_height, notok.height + 2 * uiconf.button_inner_padding);
            }
            if (self.w.cancel) |cancel| {
                button_amount += 1;
                combined_button_length += cancel.width + uiconf.horizontal_padding + 2 * uiconf.button_inner_padding;
                max_button_height = @max(max_button_height, cancel.height + 2 * uiconf.button_inner_padding);
            }

            self.width = @max(combined_button_length + uiconf.horizontal_padding, self.width);

            if (max_button_height > 0) self.height += max_button_height + uiconf.vertical_padding;

            debug.assert(self.hotspots.items.len == 0);
            try self.hotspots.ensureTotalCapacity(self.w.config.alloc, button_amount);
        }
    }

    pub fn deinit(self: *Surface) void {
        self.hotspots.deinit(self.w.config.alloc);
        self.layer_surface.destroy();
        self.wl_surface.destroy();
        self.circle.deinit(self.w.config.alloc);
    }

    pub fn hotspotFromPoint(self: *Surface, x: u31, y: u31) ?*HotSpot {
        for (self.hotspots.items) |*hs| {
            if (hs.containsPoint(x, y)) {
                return hs;
            }
        }
        return null;
    }

    fn layerSurfaceListener(layer_surface: *zwlr.LayerSurfaceV1, event: zwlr.LayerSurfaceV1.Event, self: *Surface) void {
        switch (event) {
            .configure => |ev| {
                log.debug("layer surface configure event.", .{});
                // We just ignore the requested sizes. Figuring out a good size
                // based on the text context is already complicated enough. If
                // this annoys you, patches are welcome.
                self.configured = true;
                layer_surface.ackConfigure(ev.serial);
                self.render() catch {
                    self.w.abort(error.OutOfMemory);
                    return;
                };
            },
            .closed => {
                log.debug("layer surface closed by server.", .{});
                self.w.abort(error.OutOfMemory);
            },
        }
    }

    fn render(self: *Surface) !void {
        if (!self.configured) return;
        log.debug("render.", .{});

        const uiconf = self.w.config.wayland_ui;
        const colours = self.w.config.wayland_colours;

        const buffer = try self.w.buffer_pool.nextBuffer(self.w, self.width, self.height);
        const image = buffer.*.pixman_image.?;

        self.drawBackground(image, self.width, self.height);

        var Y: u31 = uiconf.vertical_padding;
        if (self.w.title) |title| {
            const X = @divFloor(self.width, 2) -| @divFloor(title.width, 2);
            Y += try title.draw(image, &colours.text, X, Y, uiconf.vertical_padding);
        }
        if (self.w.description) |description| {
            const X = @divFloor(self.width, 2) -| @divFloor(description.width, 2);
            Y += try description.draw(image, &colours.text, X, Y, uiconf.vertical_padding);
        }

        if (self.w.mode == .getpin) {
            if (self.w.prompt) |prompt| {
                const X = @divFloor(self.width, 2) -| @divFloor(prompt.width, 2);
                Y += try prompt.draw(image, &colours.text, X, Y, uiconf.vertical_padding);
            }
            Y += self.drawPinArea(image, self.w.config.secbuf.len, Y);
        }

        if (self.w.errmessage) |errmessage| {
            const X = @divFloor(self.width, 2) -| @divFloor(errmessage.width, 2);
            Y += try errmessage.draw(image, &colours.error_text, X, Y, uiconf.vertical_padding);
        }

        // The hotspot list is populated on first render. We could technically
        // do it in calculateSize(), but we already have all the sizes here
        // already, so doing it here is more convenient for now.
        const populate_hotspots = self.hotspots.items.len == 0;

        // Buttons
        {
            const combined_button_length = blk: {
                var len: u31 = 0;
                if (self.w.ok) |ok| len += ok.width + uiconf.horizontal_padding + 2 * uiconf.button_inner_padding;
                if (self.w.notok) |notok| len += notok.width + uiconf.horizontal_padding + 2 * uiconf.button_inner_padding;
                if (self.w.cancel) |cancel| len += cancel.width + uiconf.horizontal_padding + 2 * uiconf.button_inner_padding;
                break :blk len;
            };
            var X: u31 = @divFloor(self.width + uiconf.horizontal_padding, 2) -| @divFloor(combined_button_length, 2);
            if (self.w.cancel) |cancel| {
                if (populate_hotspots) {
                    self.hotspots.appendAssumeCapacity(.{
                        .effect = .cancel,
                        .x = X,
                        .y = Y,
                        .width = cancel.width + 2 * uiconf.button_inner_padding,
                        .height = cancel.height + 2 * uiconf.button_inner_padding,
                    });
                }

                borderedRectangle(
                    image,
                    X,
                    Y,
                    cancel.width + 2 * uiconf.button_inner_padding,
                    cancel.height + 2 * uiconf.button_inner_padding,
                    uiconf.button_border,
                    self.scale,
                    &colours.cancel_button,
                    &colours.cancel_button_border,
                );
                _ = try cancel.draw(
                    image,
                    &colours.cancel_button_text,
                    X + uiconf.button_inner_padding,
                    Y + uiconf.button_inner_padding,
                    uiconf.vertical_padding,
                );
                X += cancel.width + 2 * uiconf.button_inner_padding + uiconf.horizontal_padding;
            }
            if (self.w.notok) |notok| {
                if (populate_hotspots) {
                    self.hotspots.appendAssumeCapacity(.{
                        .effect = .notok,
                        .x = X,
                        .y = Y,
                        .width = notok.width + 2 * uiconf.button_inner_padding,
                        .height = notok.height + 2 * uiconf.button_inner_padding,
                    });
                }

                borderedRectangle(
                    image,
                    X,
                    Y,
                    notok.width + 2 * uiconf.button_inner_padding,
                    notok.height + 2 * uiconf.button_inner_padding,
                    uiconf.button_border,
                    self.scale,
                    &colours.not_ok_button,
                    &colours.not_ok_button_border,
                );
                _ = try notok.draw(
                    image,
                    &colours.not_ok_button_text,
                    X + uiconf.button_inner_padding,
                    Y + uiconf.button_inner_padding,
                    uiconf.vertical_padding,
                );
                X += notok.width + 2 * uiconf.button_inner_padding + uiconf.horizontal_padding;
            }
            if (self.w.ok) |ok| {
                if (populate_hotspots) {
                    self.hotspots.appendAssumeCapacity(.{
                        .effect = .ok,
                        .x = X,
                        .y = Y,
                        .width = ok.width + 2 * uiconf.button_inner_padding,
                        .height = ok.height + 2 * uiconf.button_inner_padding,
                    });
                }

                borderedRectangle(
                    image,
                    X,
                    Y,
                    ok.width + 2 * uiconf.button_inner_padding,
                    ok.height + 2 * uiconf.button_inner_padding,
                    uiconf.button_border,
                    self.scale,
                    &colours.ok_button,
                    &colours.ok_button_border,
                );
                _ = try ok.draw(
                    image,
                    &colours.ok_button_text,
                    X + uiconf.button_inner_padding,
                    Y + uiconf.button_inner_padding,
                    uiconf.vertical_padding,
                );
            }
        }

        self.wl_surface.setBufferScale(self.scale);
        self.wl_surface.attach(buffer.*.wl_buffer.?, 0, 0);
        self.wl_surface.damageBuffer(0, 0, math.maxInt(i31), math.maxInt(u31));
        self.wl_surface.commit();
        buffer.*.busy = true;
    }

    fn drawBackground(self: *Surface, image: *pixman.Image, width: u31, height: u31) void {
        const uiconf = self.w.config.wayland_ui;
        const colours = self.w.config.wayland_colours;
        borderedRectangle(
            image,
            0,
            0,
            width,
            height,
            uiconf.border,
            self.scale,
            &colours.background,
            &colours.border,
        );

        if (uiconf.corner_radius > 0) {
            const corner_radius = @min(
                uiconf.corner_radius,
                @divFloor(self.width, 2),
                @divFloor(self.height, 2),
            );
            const colour_source_background = pixman.Image.createSolidFill(&colours.background);
            defer {
                if (colour_source_background) |b| _ = b.unref();
            }
            const colour_source_border = pixman.Image.createSolidFill(&colours.border);
            defer {
                if (colour_source_border) |b| _ = b.unref();
            }

            if (colour_source_background != null) {
                pixman.Image.composite32(
                    .src,
                    colour_source_background.?,
                    self.circle.background.?,
                    image,
                    0, // Source coords.
                    0,
                    0, // Mask coords.
                    0,
                    0, // Destination coords.
                    0,
                    corner_radius, // Source dimensions.
                    corner_radius,
                );
                pixman.Image.composite32(
                    .src,
                    colour_source_background.?,
                    self.circle.background.?,
                    image,
                    0, // Source coords.
                    0,
                    corner_radius, // Mask coords.
                    0,
                    width - corner_radius, // Destination coords.
                    0,
                    corner_radius, // Source dimensions.
                    corner_radius,
                );
                pixman.Image.composite32(
                    .src,
                    colour_source_background.?,
                    self.circle.background.?,
                    image,
                    0, // Source coords.
                    0,
                    0, // Mask coords.
                    corner_radius,
                    0, // Destination coords.
                    height - corner_radius,
                    corner_radius, // Source dimensions.
                    corner_radius,
                );
                pixman.Image.composite32(
                    .src,
                    colour_source_background.?,
                    self.circle.background.?,
                    image,
                    0, // Source coords.
                    0,
                    corner_radius, // Mask coords.
                    corner_radius,
                    width - corner_radius, // Destination coords.
                    height - corner_radius,
                    corner_radius, // Source dimensions.
                    corner_radius,
                );
            }

            if (colour_source_border != null) {
                pixman.Image.composite32(
                    .over,
                    colour_source_border.?,
                    self.circle.outline.?,
                    image,
                    0, // Source coords.
                    0,
                    0, // Mask coords.
                    0,
                    0, // Destination coords.
                    0,
                    corner_radius, // Source dimensions.
                    corner_radius,
                );
                pixman.Image.composite32(
                    .over,
                    colour_source_border.?,
                    self.circle.outline.?,
                    image,
                    0, // Source coords.
                    0,
                    corner_radius, // Mask coords.
                    0,
                    width - corner_radius, // Destination coords.
                    0,
                    corner_radius, // Source dimensions.
                    corner_radius,
                );
                pixman.Image.composite32(
                    .over,
                    colour_source_border.?,
                    self.circle.outline.?,
                    image,
                    0, // Source coords.
                    0,
                    0, // Mask coords.
                    corner_radius,
                    0, // Destination coords.
                    height - corner_radius,
                    corner_radius, // Source dimensions.
                    corner_radius,
                );
                pixman.Image.composite32(
                    .over,
                    colour_source_border.?,
                    self.circle.outline.?,
                    image,
                    0, // Source coords.
                    0,
                    corner_radius, // Mask coords.
                    corner_radius,
                    width - corner_radius, // Destination coords.
                    height - corner_radius,
                    corner_radius, // Source dimensions.
                    corner_radius,
                );
            }
        }
    }

    fn drawPinArea(self: *Surface, image: *pixman.Image, len: usize, pinarea_y: u31) u31 {
        const uiconf = self.w.config.wayland_ui;
        const colours = self.w.config.wayland_colours;
        const square_padding = @divFloor(uiconf.pin_square_size, 2);
        const pinarea_height = uiconf.pin_square_size + 2 * square_padding;
        const pinarea_width = uiconf.pin_square_amount * (uiconf.pin_square_size + square_padding) + square_padding;
        const pinarea_x = @divFloor(self.width, 2) - @divFloor(pinarea_width, 2);

        borderedRectangle(
            image,
            pinarea_x,
            pinarea_y,
            pinarea_width,
            pinarea_height,
            uiconf.border,
            self.scale,
            &colours.pin_background,
            &colours.pin_border,
        );

        var i: usize = 0;
        while (i < len and i < uiconf.pin_square_amount) : (i += 1) {
            const x: u31 = @intCast(pinarea_x + (i * uiconf.pin_square_size) + ((i + 1) * square_padding));
            const y = pinarea_y + square_padding;
            borderedRectangle(
                image,
                x,
                y,
                uiconf.pin_square_size,
                uiconf.pin_square_size,
                uiconf.pin_square_border,
                self.scale,
                &colours.pin_square,
                &colours.pin_border,
            );
        }

        return pinarea_height + uiconf.vertical_padding;
    }

    fn borderedRectangle(
        image: *pixman.Image,
        _x: u31,
        _y: u31,
        _width: u31,
        _height: u31,
        _border: u31,
        scale: u31,
        background_colour: *const pixman.Color,
        border_colour: *const pixman.Color,
    ) void {
        const x: i16 = @intCast(_x * scale);
        const y: i16 = @intCast(_y * scale);
        const width: u15 = @intCast(_width * scale);
        const height: u15 = @intCast(_height * scale);
        const border: u15 = @intCast(_border * scale);
        _ = pixman.Image.fillRectangles(.src, image, background_colour, 1, &[1]pixman.Rectangle16{
            .{ .x = x, .y = y, .width = width, .height = height },
        });
        _ = pixman.Image.fillRectangles(.src, image, border_colour, 4, &[4]pixman.Rectangle16{
            .{ .x = x, .y = y, .width = width, .height = border }, // Top
            .{ .x = x, .y = (y + height - border), .width = width, .height = border }, // Bottom
            .{ .x = x, .y = (y + border), .width = border, .height = (height -| 2 * border) }, // Left
            .{ .x = (x + width - border), .y = (y + border), .width = border, .height = (height -| 2 * border) }, // Right
        });
    }
};

const BufferPool = struct {
    /// The amount of buffers per surface we consider the reasonable upper limit.
    /// Some compositors sometimes tripple-buffer, so three seems to be ok.
    /// Note that we can absolutely work with higher buffer numbers if needed,
    /// however we consider that to be an anomaly and therefore do not want to
    /// keep all those extra buffers around if we can avoid it, as to not have
    /// unecessary memory overhead.
    const max_buffer_multiplicity = 3;

    /// The buffers. This is a linked list and not an array list, because we
    /// need stable pointers for the listener of the wl_buffer object.
    buffers: std.TailQueue(Buffer) = .{},

    /// Deinit the buffer pool, destroying all buffers and freeing all memory.
    pub fn deinit(self: *BufferPool, alloc: mem.Allocator) void {
        var it = self.buffers.first;
        while (it) |node| {
            // We need to get the next node before destroying the current one.
            it = node.next;
            node.data.deinit();
            alloc.destroy(node);
        }
    }

    /// Get a buffer of the specified dimenisons. If possible an idle buffer is
    /// reused, otherweise a new one is created.
    pub fn nextBuffer(self: *BufferPool, w: *Wayland, width: u31, height: u31) !*Buffer {
        log.debug("Next buffer: {}x{}; Total buffers: {}", .{ width, height, self.buffers.len });
        defer {
            if (self.buffers.len > max_buffer_multiplicity * self.globalSurfaceCount()) {
                self.cullBuffers(w);
            }
        }
        if (try self.findSuitableBuffer(w, width, height)) |buffer| {
            return buffer;
        } else {
            return try self.newBuffer(w, width, height);
        }
    }

    fn findSuitableBuffer(self: *BufferPool, w: *Wayland, width: u31, height: u31) !?*Buffer {
        var it = self.buffers.first;
        var first_unbusy_buffer_node: ?*std.TailQueue(Buffer).Node = null;
        while (it) |node| : (it = node.next) {
            if (node.data.busy) continue;
            if (node.data.width == width and node.data.height == height) {
                return &node.data;
            } else {
                first_unbusy_buffer_node = node;
            }
        }

        // No buffer has matching dimensions, however we do have an unbusy
        // buffer which we can just re-init.
        if (first_unbusy_buffer_node) |node| {
            node.data.deinit();
            try node.data.init(w, width, height);
            return &node.data;
        }

        return null;
    }

    fn newBuffer(self: *BufferPool, w: *Wayland, width: u31, height: u31) !*Buffer {
        log.debug("New buffer: {}x{}", .{ width, height });
        const alloc = w.config.alloc;
        const node = try alloc.create(std.TailQueue(Buffer).Node);
        errdefer alloc.destroy(node);
        try node.data.init(w, width, height);
        self.buffers.append(node);
        return &node.data;
    }

    fn globalSurfaceCount(_: *BufferPool) usize {
        return 1;
    }

    fn cullBuffers(self: *BufferPool, w: *Wayland) void {
        log.debug("Culling buffers.", .{});
        const alloc = w.config.alloc;
        var overhead = self.buffers.len - max_buffer_multiplicity * self.globalSurfaceCount();
        var it = self.buffers.first;
        while (it) |node| {
            if (overhead == 0) break;
            // We need to get the next node before destroying the current one.
            it = node.next;
            if (!node.data.busy) {
                node.data.deinit();
                self.buffers.remove(node);
                alloc.destroy(node);
                overhead -= 1;
            }
        }
        log.debug(" -> new buffer count: {}", .{self.buffers.len});
    }
};

// Copied and adapted from https://git.sr.ht/~novakane/zelbar, same license.
const Buffer = struct {
    wl_buffer: ?*wl.Buffer = null,
    pixman_image: ?*pixman.Image = null,
    data: ?[]align(std.mem.page_size) u8 = null,
    width: u31 = 0, // u31 can coerce to i32.
    height: u31 = 0,
    busy: bool = false,

    pub fn init(self: *Buffer, w: *Wayland, width: u31, height: u31) !void {
        const stride = width << 2;
        const size = stride * height;

        if (size == 0) return error.ZeroSizedBuffer;

        const fd = blk: {
            if (builtin.target.os.tag == .linux) {
                break :blk try posix.memfd_createZ("/wayprompt", std.os.linux.MFD.CLOEXEC);
            }
            @compileError("patches welcome");
        };
        defer posix.close(fd);
        try posix.ftruncate(fd, size);

        const data = mem.bytesAsSlice(u8, try posix.mmap(
            null,
            size,
            posix.PROT.READ | posix.PROT.WRITE,
            posix.MAP{ .TYPE = std.os.linux.MAP_TYPE.SHARED },
            fd,
            0,
        ));
        errdefer posix.munmap(data);

        const shm_pool = try w.shm.?.createPool(fd, size);
        defer shm_pool.destroy();

        const wl_buffer = try shm_pool.createBuffer(0, width, height, stride, .argb8888);
        errdefer wl_buffer.destroy();
        wl_buffer.setListener(*Buffer, bufferListener, self);

        const pixman_image = pixman.Image.createBitsNoClear(
            .a8r8g8b8,
            @as(c_int, @intCast(width)),
            @as(c_int, @intCast(height)),
            @as([*c]u32, @ptrCast(data)),
            @as(c_int, @intCast(stride)),
        );
        errdefer _ = pixman_image.unref();

        self.* = .{
            .wl_buffer = wl_buffer,
            .pixman_image = pixman_image,
            .data = data,
            .width = width,
            .height = height,
        };
    }

    fn deinit(self: Buffer) void {
        if (self.pixman_image) |p| _ = p.unref();
        if (self.wl_buffer) |wb| wb.destroy();
        if (self.data) |d| posix.munmap(d);
    }

    fn bufferListener(_: *wl.Buffer, event: wl.Buffer.Event, self: *Buffer) void {
        switch (event) {
            .release => self.busy = false,
        }
    }
};

config: *Config = undefined,
mode: Frontend.InterfaceMode = .none,

title: ?TextView = null,
description: ?TextView = null,
prompt: ?TextView = null,
errmessage: ?TextView = null,
ok: ?TextView = null,
notok: ?TextView = null,
cancel: ?TextView = null,
font_large: ?*fcft.Font = null,
font_regular: ?*fcft.Font = null,

display: *wl.Display = undefined,
registry: ?*wl.Registry = null,
layer_shell: ?*zwlr.LayerShellV1 = null,
cursor_shape_manager: ?*wp.CursorShapeManagerV1 = null,
compositor: ?*wl.Compositor = null,
shm: ?*wl.Shm = null,
seats: std.TailQueue(Seat) = .{},
buffer_pool: BufferPool = .{},
surface: ?Surface = null,

/// While this is not null, we have not reached the sync handler and as such
/// not bound all the globals we need. Because for some backends (like the CLI
/// one) it is more ergonomic to call enterMode() _before_ entering the event
/// loop and as such before dispatching Wayland messages. In those cases, the
/// mode gets delayed until the sync listener fires.
sync: ?*wl.Callback = null,
delayed_mode: ?Frontend.InterfaceMode = null,

xkb_context: ?*xkb.Context = undefined,

exit_reason: ?anyerror = null,

pub fn init(self: *Wayland, cfg: *Config) !posix.fd_t {
    self.config = cfg;

    const wayland_display = blk: {
        if (cfg.wayland_display) |wd| break :blk wd;
        if (posix.getenv("WAYLAND_DISPLAY")) |wd| break :blk wd;
        return error.NoWaylandDisplay;
    };
    log.debug("trying to connect to '{s}'.", .{wayland_display});

    self.display = try wl.Display.connect(@as([*:0]const u8, @ptrCast(wayland_display.ptr)));
    errdefer self.deinit();

    self.registry = try self.display.getRegistry();
    self.registry.?.setListener(*Wayland, registryListener, self);

    self.sync = try self.display.sync();
    self.sync.?.setListener(*Wayland, syncListener, self);

    self.xkb_context = xkb.Context.new(.no_flags) orelse return error.OutOfMemory;

    _ = fcft.init(.never, false, .none);

    if (self.config.wayland_ui.font_regular) |user_font| {
        var fonts = [_][*:0]const u8{ user_font, "sans:size=14", "mono:size=14" };
        self.font_regular = try fcft.Font.fromName(fonts[0..], null);
    } else {
        var fonts = [_][*:0]const u8{ "sans:size=14", "mono:size=14" };
        self.font_regular = try fcft.Font.fromName(fonts[0..], null);
    }

    if (self.config.wayland_ui.font_large) |user_font| {
        var fonts = [_][*:0]const u8{ user_font, "sans:size=20", "mono:size=20" };
        self.font_large = try fcft.Font.fromName(fonts[0..], null);
    } else {
        var fonts = [_][*:0]const u8{ "sans:size=20", "mono:size=20" };
        self.font_large = try fcft.Font.fromName(fonts[0..], null);
    }

    return self.display.getFd();
}

pub fn deinit(self: *Wayland) void {
    const alloc = self.config.alloc;

    // FCFT teardown.
    {
        self.deinitTextViews();
        if (self.font_large) |f| f.destroy();
        if (self.font_regular) |f| f.destroy();
        fcft.fini();
    }

    if (self.surface) |*s| s.deinit();

    self.buffer_pool.deinit(self.config.alloc);
    if (self.layer_shell) |ls| ls.destroy();
    if (self.cursor_shape_manager) |csm| csm.destroy();
    if (self.compositor) |cmp| cmp.destroy();
    if (self.shm) |sm| sm.destroy();

    var it = self.seats.first;
    while (it) |node| {
        it = node.next;
        node.data.deinit();
        alloc.destroy(node);
    }

    if (self.xkb_context) |x| x.unref();

    if (self.sync) |s| s.destroy();
    if (self.registry) |r| r.destroy();
    self.display.disconnect();
}

pub fn enterMode(self: *Wayland, mode: Frontend.InterfaceMode) !void {
    if (self.mode == mode) {
        debug.assert(self.mode == .none);
        return;
    }

    // See doc-comment for Wayland.sync.
    if (self.sync != null) {
        log.debug("trying to enter mode but haven't sync'd yet, delaying.", .{});
        self.delayed_mode = mode;
        return;
    }

    self.deinitTextViews();

    self.mode = mode;
    if (mode == .none) {
        debug.assert(self.surface != null);
        self.surface.?.deinit();
        self.surface = null;
    } else {
        try self.initTextViews();
        self.surface = Surface{};
        self.surface.?.init(self) catch |err| {
            log.err("failed to init surface: {s}", .{@errorName(err)});
            self.surface = null;
            self.abort(error.OutOfMemory);
        };
    }
}

/// Note: this depends on labels and corresponding TextViews having the same field name.
fn initTextViews(self: *Wayland) !void {
    const alloc = self.config.alloc;
    const labels = self.config.labels;
    if (labels.title) |title| self.title = try TextView.new(alloc, mem.trim(u8, title, &ascii.whitespace), self.font_large.?);
    if (labels.description) |description| self.description = try TextView.new(alloc, mem.trim(u8, description, &ascii.whitespace), self.font_regular.?);
    if (labels.err_message) |errmessage| self.errmessage = try TextView.new(alloc, mem.trim(u8, errmessage, &ascii.whitespace), self.font_regular.?);
    if (labels.prompt) |prompt| self.prompt = try TextView.new(alloc, mem.trim(u8, prompt, &ascii.whitespace), self.font_large.?);
    if (labels.ok) |ok| self.ok = try TextView.new(alloc, mem.trim(u8, ok, &ascii.whitespace), self.font_regular.?);
    if (labels.not_ok) |notok| self.notok = try TextView.new(alloc, mem.trim(u8, notok, &ascii.whitespace), self.font_regular.?);
    if (labels.cancel) |cancel| self.cancel = try TextView.new(alloc, mem.trim(u8, cancel, &ascii.whitespace), self.font_regular.?);
}

fn deinitTextViews(self: *Wayland) void {
    const alloc = self.config.alloc;
    if (self.title) |title| {
        self.title = null;
        title.deinit(alloc);
    }
    if (self.description) |description| {
        self.description = null;
        description.deinit(alloc);
    }
    if (self.errmessage) |errmessage| {
        self.errmessage = null;
        errmessage.deinit(alloc);
    }
    if (self.prompt) |prompt| {
        self.prompt = null;
        prompt.deinit(alloc);
    }
    if (self.ok) |ok| {
        self.ok = null;
        ok.deinit(alloc);
    }
    if (self.notok) |notok| {
        self.notok = null;
        notok.deinit(alloc);
    }
    if (self.cancel) |cancel| {
        self.cancel = null;
        cancel.deinit(alloc);
    }
}

/// Flushes Wayland events and prepares read, needed for handleEvent().
pub fn flush(self: *Wayland) !Frontend.Event {
    while (!self.display.prepareRead()) {
        const errno = self.display.dispatchPending();
        if (errno != .SUCCESS) {
            log.err("failed to dispatch pending Wayland events: {s}", .{@tagName(errno)});
            self.abort(error.UnexpectedError);
        }
    }

    while (true) {
        const errno = self.display.flush();
        switch (errno) {
            .SUCCESS => break,
            .PIPE => _ = self.display.readEvents(), // Server closed connection.
            .AGAIN => {
                log.debug("EAGAIN during Wayland display flush.", .{});
                continue;
            },
            else => {
                log.err("flushing Wayland display failed: {s}", .{@tagName(errno)});
                self.abort(error.UnexpectedError);
                break;
            },
        }
    }

    if (self.exit_reason) |er| {
        return try self.exitReasonToReturnVal(er);
    } else {
        return .none;
    }
}

pub fn handleEvent(self: *Wayland) !Frontend.Event {
    {
        const errno = self.display.readEvents();
        if (errno != .SUCCESS) {
            log.err("reading Wayland display failed: {s}", .{@tagName(errno)});
            self.abort(error.UnexpectedError);
        }
    }

    {
        const errno = self.display.dispatchPending();
        if (errno != .SUCCESS) {
            log.err("failed to dispatch pending Wayland events: {s}", .{@tagName(errno)});
            self.abort(error.UnexpectedError);
        }
    }

    if (self.exit_reason) |er| {
        return try self.exitReasonToReturnVal(er);
    } else {
        return .none;
    }
}

pub fn noEvent(self: *Wayland) !void {
    self.display.cancelRead();
}

fn exitReasonToReturnVal(self: *Wayland, er: anyerror) !Frontend.Event {
    // The first three are not technically errors, but using errors for these
    // cases allows us to handle them a tad bit nicer.
    switch (er) {
        error.UserAbort, error.UserNotOk, error.UserOk => {
            self.exit_reason = null;
            try self.enterMode(.none);
            switch (er) {
                error.UserAbort => return .user_abort,
                error.UserNotOk => return .user_notok,
                error.UserOk => return .user_ok,
                else => unreachable,
            }
        },
        else => return er,
    }
}

fn abort(self: *Wayland, reason: anyerror) void {
    switch (reason) {
        error.UserAbort, error.UserNotOk, error.UserOk => {},
        else => log.err("aborting: {s}", .{@errorName(reason)}),
    }
    self.exit_reason = reason;
}

fn registryListener(registry: *wl.Registry, event: wl.Registry.Event, self: *Wayland) void {
    switch (event) {
        .global => |ev| {
            if (mem.orderZ(u8, ev.interface, zwlr.LayerShellV1.getInterface().name) == .eq) {
                self.layer_shell = registry.bind(ev.name, zwlr.LayerShellV1, 4) catch {
                    self.abort(error.OutOfMemory);
                    return;
                };
            } else if (mem.orderZ(u8, ev.interface, wp.CursorShapeManagerV1.getInterface().name) == .eq) {
                self.cursor_shape_manager = registry.bind(ev.name, wp.CursorShapeManagerV1, 1) catch {
                    self.abort(error.OutOfMemory);
                    return;
                };
            } else if (mem.orderZ(u8, ev.interface, wl.Compositor.getInterface().name) == .eq) {
                self.compositor = registry.bind(ev.name, wl.Compositor, 4) catch {
                    self.abort(error.OutOfMemory);
                    return;
                };
            } else if (mem.orderZ(u8, ev.interface, wl.Shm.getInterface().name) == .eq) {
                self.shm = registry.bind(ev.name, wl.Shm, 1) catch {
                    self.abort(error.OutOfMemory);
                    return;
                };
            } else if (mem.orderZ(u8, ev.interface, wl.Seat.getInterface().name) == .eq) {
                const seat = registry.bind(ev.name, wl.Seat, 1) catch {
                    self.abort(error.OutOfMemory);
                    return;
                };
                self.addSeat(seat) catch {
                    seat.destroy();
                    self.abort(error.OutOfMemory);
                };
            }
        },
        .global_remove => {}, // We do not live long enough for this to become relevant.
    }
}

fn addSeat(self: *Wayland, wl_seat: *wl.Seat) !void {
    const node = try self.config.alloc.create(std.TailQueue(Seat).Node);
    try node.data.init(self, wl_seat);
    self.seats.append(node);
}

fn syncListener(_: *wl.Callback, _: wl.Callback.Event, self: *Wayland) void {
    log.debug("sync listener reached.", .{});

    if (self.layer_shell == null or self.compositor == null or self.shm == null) {
        self.abort(error.MissingWaylandInterfaces);
    }

    if (self.sync) |s| s.destroy();
    self.sync = null;

    // See doc-comment for Wayland.sync.
    if (self.delayed_mode) |mode| {
        log.debug("delayed mode found, entering.", .{});
        self.delayed_mode = null;
        self.enterMode(mode) catch |err| {
            self.abort(err);
        };
    }
}
