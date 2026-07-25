const std = @import("std");
const fs = std.fs;
const mem = std.mem;

const Scanner = @import("zig-wayland").Scanner;

pub fn build(b: *std.Build) !void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});
    const options = b.addOptions();
    const strip = b.option(bool, "strip", "Omit debug information") orelse false;
    const pie = b.option(bool, "pie", "Build with PIE support (by default false)") orelse false;
    const llvm = !(b.option(bool, "no-llvm", "(experimental) Use non-LLVM x86 Zig backend") orelse false);

    const scanner = Scanner.create(b, .{});
    scanner.addCustomProtocol("protocol/wlr-layer-shell-unstable-v1.xml");
    scanner.addSystemProtocol("stable/xdg-shell/xdg-shell.xml"); // Dependency of layer-shell.
    scanner.addSystemProtocol("staging/cursor-shape/cursor-shape-v1.xml");
    scanner.addSystemProtocol("unstable/tablet/tablet-unstable-v2.xml"); // Dependency of cursor-shape.
    scanner.generate("wp_cursor_shape_manager_v1", 1);
    scanner.generate("wl_compositor", 5);
    scanner.generate("wl_shm", 1);
    scanner.generate("zwlr_layer_shell_v1", 3);
    scanner.generate("wl_seat", 8);
    scanner.generate("wl_output", 4);

    const wayland = b.createModule(.{ .root_source_file = scanner.result });
    const xkbcommon = b.dependency("zig-xkbcommon", .{}).module("xkbcommon");
    const pixman = b.dependency("zig-pixman", .{}).module("pixman");
    const spoon = b.dependency("zig-spoon", .{}).module("spoon");
    const fcft = b.dependency("zig-fcft", .{}).module("fcft");
    const ini = b.dependency("zig-ini", .{}).module("ini");

    const wayprompt_cli = b.addExecutable(.{
        .name = "wayprompt",
        .root_source_file = b.path("src/wayprompt-cli.zig"),
        .target = target,
        .optimize = optimize,
        .strip = strip,
        .use_llvm = llvm,
        .use_lld = llvm,
    });
    wayprompt_cli.root_module.addOptions("build_options", options);
    wayprompt_cli.linkLibC();
    wayprompt_cli.root_module.addImport("wayland", wayland);
    wayprompt_cli.linkSystemLibrary("wayland-client");
    wayprompt_cli.linkSystemLibrary("wayland-cursor");
    scanner.addCSource(wayprompt_cli);
    wayprompt_cli.root_module.addImport("ini", ini);
    wayprompt_cli.root_module.addImport("fcft", fcft);
    wayprompt_cli.linkSystemLibrary("fcft");
    wayprompt_cli.root_module.addImport("xkbcommon", xkbcommon);
    wayprompt_cli.linkSystemLibrary("xkbcommon");
    wayprompt_cli.root_module.addImport("pixman", pixman);
    wayprompt_cli.linkSystemLibrary("pixman-1");
    wayprompt_cli.root_module.addImport("spoon", spoon);
    wayprompt_cli.root_module.addOptions("build_options", options);
    wayprompt_cli.pie = pie;
    b.installArtifact(wayprompt_cli);

    const wayprompt_pinentry = b.addExecutable(.{
        .name = "pinentry-wayprompt",
        .root_source_file = b.path("src/wayprompt-pinentry.zig"),
        .target = target,
        .optimize = optimize,
        .strip = strip,
        .use_llvm = llvm,
        .use_lld = llvm,
    });
    wayprompt_pinentry.linkLibC();
    wayprompt_pinentry.root_module.addImport("wayland", wayland);
    wayprompt_pinentry.linkSystemLibrary("wayland-client");
    wayprompt_pinentry.linkSystemLibrary("wayland-cursor");
    scanner.addCSource(wayprompt_pinentry);
    wayprompt_pinentry.root_module.addImport("ini", ini);
    wayprompt_pinentry.root_module.addImport("fcft", fcft);
    wayprompt_pinentry.linkSystemLibrary("fcft");
    wayprompt_pinentry.root_module.addImport("xkbcommon", xkbcommon);
    wayprompt_pinentry.linkSystemLibrary("xkbcommon");
    wayprompt_pinentry.root_module.addImport("pixman", pixman);
    wayprompt_pinentry.linkSystemLibrary("pixman-1");
    wayprompt_pinentry.root_module.addImport("spoon", spoon);
    wayprompt_pinentry.root_module.addOptions("build_options", options);
    wayprompt_pinentry.pie = pie;
    b.installArtifact(wayprompt_pinentry);

    const tests = b.addTest(.{
        .root_source_file = b.path("src/tests.zig"),
        .target = target,
        .optimize = optimize,
    });
    const run_test = b.addRunArtifact(tests);
    const test_step = b.step("test", "Run tests");
    test_step.dependOn(&run_test.step);

    b.installFile("bin/wayprompt-ssh-askpass", "bin/wayprompt-ssh-askpass");

    b.installFile("doc/wayprompt.1", "share/man/man1/wayprompt.1");
    b.installFile("doc/pinentry-wayprompt.1", "share/man/man1/pinentry-wayprompt.1");
    b.installFile("doc/wayprompt-ssh-askpass.1", "share/man/man1/wayprompt-ssh-askpass.1");
    b.installFile("doc/wayprompt.5", "share/man/man5/wayprompt.5");
}
