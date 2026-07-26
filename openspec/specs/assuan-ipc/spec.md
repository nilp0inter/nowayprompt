## Purpose

Defines the synchronous Assuan pinentry protocol and command state machine.

## Requirements

### Requirement: Assuan wire framing and greeting

The Assuan IPC handler MUST run a synchronous line-framed REPL over stdin/stdout. On startup, it MUST emit the greeting `OK wayprompt is pleased to meet you\n` and flush before reading any command. All frames are terminated by `\n`. The handler MUST NOT use async runtimes.

#### Scenario: Startup greeting
- **WHEN** the pinentry process starts
- **THEN** stdout receives `OK wayprompt is pleased to meet you\n` and is flushed before the first command is read

#### Scenario: Synchronous REPL
- **WHEN** a command line is read from stdin
- **THEN** the handler dispatches it synchronously and writes the response before reading the next line; no `tokio`/`async-std`/`futures` runtime is linked

### Requirement: Percent-decoding and hotkey-underscore stripping

The module MUST provide `assuan_decode(input: &str, strip_hotkey: bool) -> Result<String, AssuanError>` mirroring legacy `pinentryDupe`. `%XX` sequences (where `XX` is hex) MUST decode to the byte `u8::from_str_radix(XX, 16)`. When `strip_hotkey` is `true`, leading-underscore hotkey markers (`_`) MUST be stripped (skipped, not decoded). Malformed percent escapes (fewer than 2 trailing bytes, or non-hex digits) MUST return an error. The output MUST be valid UTF-8. `strip_hotkey=true` is applied ONLY to `default-ok`, `default-cancel`, `default-yes`, `default-no` `OPTION` values; all `SET*` commands use `strip_hotkey=false`.

#### Scenario: Percent-decode a space
- **WHEN** `assuan_decode("foo%20bar", false)` is called
- **THEN** the result is `Ok("foo bar")`

#### Scenario: Strip hotkey underscore
- **WHEN** `assuan_decode("_Cancel", true)` is called
- **THEN** the result is `Ok("Cancel")`

#### Scenario: Keep underscore when strip_hotkey is false
- **WHEN** `assuan_decode("_Cancel", false)` is called
- **THEN** the result is `Ok("_Cancel")`

#### Scenario: Malformed percent escape
- **WHEN** `assuan_decode("foo%2", false)` is called (only 1 hex digit)
- **THEN** the result is an `Err`

#### Scenario: Invalid hex digits
- **WHEN** `assuan_decode("foo%ZZbar", false)` is called
- **THEN** the result is an `Err`

### Requirement: SETTITLE, SETPROMPT, SETDESC, SETERROR, SETOK, SETNOTOK, SETCANCEL commands

Each command MUST decode its argument with `assuan_decode(args, false)` and store the result in the corresponding `Config::labels` field (`title`, `prompt`, `description`, `err_message`, `ok`, `not_ok`, `cancel`). Any prior value in the target field MUST be freed/replaced before storing the new value. On success, the handler MUST emit `OK\n`. Commands are matched case-insensitively.

#### Scenario: SETTITLE sets the title label
- **WHEN** `SETTITLE My Title\n` is received
- **THEN** `config.labels.title` is `Some("My Title")` and stdout emits `OK\n`

#### Scenario: SETERROR sets the error label
- **WHEN** `SETERROR Bad%20PIN\n` is received
- **THEN** `config.labels.err_message` is `Some("Bad PIN")` and stdout emits `OK\n`

#### Scenario: SETOK overwrites a prior value
- **WHEN** `config.labels.ok` is `Some("old")` and `SETOK New\n` is received
- **THEN** `config.labels.ok` is `Some("New")` (the old value is replaced) and stdout emits `OK\n`

#### Scenario: Case-insensitive command match
- **WHEN** `settitle X\n` is received
- **THEN** `config.labels.title` is `Some("X")` and stdout emits `OK\n`

### Requirement: GETPIN command and zero-copy secret streaming

On `GETPIN`, the handler MUST set the Assuan-level mode to `getpin`, apply default button labels (`default_ok` → `config.labels.ok` if `ok` is `None`; `default_cancel` → `config.labels.cancel` if `cancel` is `None`), call `frontend.enter_mode(GetPin)`, and block in the dispatch loop until a frontend `Event` arrives. On `UserOk`: if `SecretBuffer::slice()` is `Some(bytes)`, emit `D ` then write the raw secret bytes directly to stdout via `write_all`, then `\nEND\nOK\n`; if `slice()` is `None`, emit `OK\n` only (no `D`/`END`). On `UserAbort`: emit `ERR 83886179 Operation cancelled\n`. On `UserNotOk`: emit `ERR 83886194 not confirmed\n`. After any terminal event in `getpin` mode, `config.labels.err_message` MUST be cleared to `None` and `SecretBuffer::reset()` MUST be called.

The `D <secret>` output MUST NOT copy the secret into a `String` or `Vec` on the heap; the secret bytes MUST stream directly from `SecretBuffer::slice()` to the stdout writer.

#### Scenario: GETPIN with non-empty secret
- **WHEN** `GETPIN\n` is received and the user enters `"hunter2"` and presses Enter
- **THEN** stdout emits `D hunter2\nEND\nOK\n` and the secret buffer is reset

#### Scenario: GETPIN with empty secret
- **WHEN** `GETPIN\n` is received and the user presses Enter on an empty prompt
- **THEN** stdout emits `OK\n` (no `D`/`END` frame) and the secret buffer is reset

#### Scenario: GETPIN cancelled by user
- **WHEN** `GETPIN\n` is received and the user presses Escape
- **THEN** stdout emits `ERR 83886179 Operation cancelled\n` and the secret buffer is reset and `err_message` is cleared

#### Scenario: GETPIN not-ok
- **WHEN** `GETPIN\n` is received and `config.labels.not_ok` is `Some` and the user presses Ctrl+C
- **THEN** stdout emits `ERR 83886194 not confirmed\n` and the secret buffer is reset and `err_message` is cleared

#### Scenario: Zero-copy secret streaming
- **WHEN** the secret is streamed on `UserOk`
- **THEN** no `String`, `Vec<u8>`, or `format!` allocation holds the secret; the bytes are written directly from `SecretBuffer::slice()` to the stdout `BufWriter`

### Requirement: CONFIRM and MESSAGE commands

`CONFIRM` MUST apply default button labels (`default_yes` → `config.labels.ok` if `ok` is `None`; `default_no` → `config.labels.cancel` if `cancel` is `None`), set Assuan mode to `confirm`, and call `frontend.enter_mode(Message)` (legacy collapses confirm/message to the same frontend mode). `MESSAGE` MUST short-circuit with `OK\n` if `title`, `description`, AND `err_message` are all `None`; otherwise set mode to `message` and call `frontend.enter_mode(Message)`. On `UserOk`: emit `OK\n`. On `UserAbort`: emit `ERR 83886179 Operation cancelled\n`. On `UserNotOk`: emit `ERR 83886194 not confirmed\n`. After any terminal event, clear `err_message` and reset the secret buffer.

#### Scenario: MESSAGE with no content
- **WHEN** `MESSAGE\n` is received and `title`, `description`, and `err_message` are all `None`
- **THEN** stdout emits `OK\n` and no frontend mode is entered

#### Scenario: MESSAGE with a description
- **WHEN** `MESSAGE\n` is received and `description` is `Some("Hello")`
- **THEN** the frontend enters `Message` mode and the loop waits for a frontend event

#### Scenario: CONFIRM applies default-yes label
- **WHEN** `CONFIRM\n` is received and `default_yes` is `Some("Yes")` and `config.labels.ok` is `None`
- **THEN** `config.labels.ok` becomes `Some("Yes")` and `default_yes` is consumed (set to `None`)

### Requirement: GETINFO command

`GETINFO <sub>` MUST respond based on the subcommand (matched case-insensitively): `flavor` → `D wayprompt\nEND\n` then `OK\n`; `version` → `D 0.0.0\nEND\n` then `OK\n`; `pid` → `D <pid>\nEND\n` then `OK\n` where `<pid>` is `std::process::id()`; any other or missing subcommand → `OK\n` only.

#### Scenario: GETINFO flavor
- **WHEN** `GETINFO flavor\n` is received
- **THEN** stdout emits `D wayprompt\nEND\nOK\n`

#### Scenario: GETINFO pid
- **WHEN** `GETINFO pid\n` is received
- **THEN** stdout emits `D <current_pid>\nEND\nOK\n` where `<current_pid>` matches `std::process::id()`

#### Scenario: GETINFO unknown subcommand
- **WHEN** `GETINFO bogus\n` is received
- **THEN** stdout emits `OK\n` only (no `D`/`END`)

### Requirement: OPTION command parsing

`OPTION <arg>` MUST parse `arg` by prefix-matching against known option prefixes (legacy `getOption` uses `mem.startsWith` on the whole option token, not `split_once('=')`): `putenv=WAYLAND_DISPLAY=` → set `config.wayland_display`; `ttyname=` → set `config.tty_name`; `default-ok=` → set runtime `default_ok` (decoded with `strip_hotkey=true`); `default-cancel=` → set `default_cancel` (strip_hotkey=true); `default-yes=` → set `default_yes` (strip_hotkey=true); `default-no=` → set `default_no` (strip_hotkey=true). Any prior value MUST be freed/replaced. Unknown options MUST be silently accepted with `OK\n` (legacy comment lines 364–368: "Most options are internationalisation for features we don't offer"). On any `OPTION`, emit `OK\n`.

#### Scenario: OPTION ttyname sets config.tty_name
- **WHEN** `OPTION ttyname=/dev/tty3\n` is received
- **THEN** `config.tty_name` is `Some("/dev/tty3")` and stdout emits `OK\n`

#### Scenario: OPTION default-ok with hotkey strip
- **WHEN** `OPTION default-ok=_Confirm\n` is received
- **THEN** the runtime `default_ok` is `Some("Confirm")` (underscore stripped) and stdout emits `OK\n`

#### Scenario: OPTION unknown is silently accepted
- **WHEN** `OPTION allow-external-password-cache\n` is received
- **THEN** stdout emits `OK\n` (no error, no state change)

### Requirement: BYE, RESET, NOP, HELP commands

`BYE` MUST emit `OK\n`, stop the REPL loop (the dispatch loop exits after the current iteration), and the process exits after cleanup. `RESET` MUST call `Config::reset()` (clears all `labels` fields to `None`) and emit `OK\n`. `NOP` MUST emit `OK\n`. `HELP` MUST emit a comment block listing `NOP`, `SETTITLE`, `SETPROMPT`, `SETDESC`, `SETERROR`, `GETPIN`, `BYE`, `OPTION`, `RESET` (each prefixed `# `) followed by `OK\n`.

#### Scenario: BYE stops the loop
- **WHEN** `BYE\n` is received
- **THEN** stdout emits `OK\n` and the REPL loop exits on the next iteration

#### Scenario: RESET clears labels
- **WHEN** `RESET\n` is received and `config.labels.title` is `Some("X")`
- **THEN** all `config.labels` fields are `None` and stdout emits `OK\n`

#### Scenario: NOP responds OK
- **WHEN** `NOP\n` is received
- **THEN** stdout emits `OK\n`

#### Scenario: HELP lists commands
- **WHEN** `HELP\n` is received
- **THEN** stdout emits `# NOP\n# SETTITLE\n# SETPROMPT\n# SETDESC\n# SETERROR\n# GETPIN\n# BYE\n# OPTION\n# RESET\nOK\n`

### Requirement: Silently-accepted command set

The handler MUST accept `SETKEYINFO` and respond `OK\n` without storing or acting on the argument. This is an interop requirement: `gpg-agent` aborts if `SETKEYINFO` is rejected. The argument MUST be consumed and discarded.

#### Scenario: SETKEYINFO is silently accepted
- **WHEN** `SETKEYINFO X:12345\n` is received
- **THEN** stdout emits `OK\n` and no state is modified

### Requirement: Not-implemented command set

The handler MUST respond `ERR 536870981 Not implemented\n` to: `CANCEL`, `SETGENPIN`, `SETGENPIN_TT`, `SETTIMEOUT`, `END`, `QUIT`, `AUTH`, `CLEARPASSPHRASE`, `SETREPEAT`, `SETREPEATERROR`, `SETQUALITYBAR`, `SETQUALITYBAR_TT`. These commands are matched case-insensitively. Arguments (if any) are ignored.

#### Scenario: SETTIMEOUT is not implemented
- **WHEN** `SETTIMEOUT 30\n` is received
- **THEN** stdout emits `ERR 536870981 Not implemented\n`

#### Scenario: SETREPEAT is not implemented
- **WHEN** `SETREPEAT\n` is received
- **THEN** stdout emits `ERR 536870981 Not implemented\n`

### Requirement: Unknown command fallback

Any command not in the implemented, silently-accepted, or not-implemented sets MUST produce `ERR 536871187 Unknown IPC command\n`.

#### Scenario: Unknown command
- **WHEN** `BOGUS arg\n` is received
- **THEN** stdout emits `ERR 536871187 Unknown IPC command\n`

### Requirement: Commands ignored while a prompt is active

The handler MUST ignore all commands received while an Assuan-level mode is active (`getpin`, `confirm`, `message`) — legacy line 276: `if (mode != .none) return;`. The command is silently dropped (no response) and the loop continues waiting for the frontend event to complete the active prompt.

#### Scenario: SETTITLE during GETPIN is ignored
- **WHEN** `GETPIN\n` has been sent and the user has not yet pressed Enter, and `SETTITLE X\n` arrives
- **THEN** `SETTITLE X\n` is silently dropped (no response, no state change)

### Requirement: Partial-line stdin buffering

The handler MUST buffer partial lines from stdin across `read` calls using `std::io::BufReader<std::io::Stdin>` (or equivalent line-accumulating reader). A command split across two reads MUST be reassembled before dispatch. This fixes legacy's open partial-line TODO while preserving full-line semantics.

#### Scenario: Command split across two reads
- **WHEN** stdin yields `SETT` on one read and `ITLE X\n` on the next read
- **THEN** the handler dispatches `SETTITLE X` as a single command and emits `OK\n`

#### Scenario: Multiple commands in one read
- **WHEN** stdin yields `NOP\nNOP\n` in a single read
- **THEN** the handler dispatches two `NOP` commands and emits `OK\n` twice