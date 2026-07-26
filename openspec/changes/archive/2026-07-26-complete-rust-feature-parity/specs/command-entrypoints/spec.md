## ADDED Requirements

### Requirement: Basename-selected public entrypoints

The installed Rust executable MUST select its public mode from its invocation
basename. `nowayprompt` MUST provide the interactive CLI contract,
`pinentry-nowayprompt` MUST provide the GPG Assuan pinentry contract, and
`nowayprompt-ssh-askpass` MUST provide the SSH askpass contract. An unknown
basename MUST fail without displaying or emitting a secret.

#### Scenario: pinentry alias starts an Assuan session
- **WHEN** the executable is invoked as `pinentry-nowayprompt`
- **THEN** it emits the Assuan greeting and processes the existing Assuan
  command contract

#### Scenario: CLI basename starts prompt mode
- **WHEN** the executable is invoked as `nowayprompt` with valid CLI options
- **THEN** it validates the CLI request, displays the requested prompt, and
  emits the legacy CLI status and exit code

#### Scenario: unknown basename is rejected
- **WHEN** the executable is invoked through an unrecognized basename
- **THEN** it exits nonzero without initializing a frontend or emitting secret
  bytes

### Requirement: CLI prompt contract

The CLI mode MUST accept `--title`, `--description`, `--prompt`, `--error`,
`--button-ok`, `--button-not-ok`, `--button-cancel`, `--wayland-display`,
`--get-pin`, `--json`, `--help`, and `-h`. It MUST reject unknown, repeated,
and argument-less value options with exit status `1`. `--prompt` MUST be
rejected unless `--get-pin` is supplied. A non-secret message request MUST
supply at least one of title, description, or error text.

On `UserOk`, CLI mode MUST emit `user-action: ok`; on `UserAbort`, it MUST emit
`user-action: cancel`; on `UserNotOk`, it MUST emit `user-action: not-ok`. In
secret mode, it MUST additionally emit `pin: <secret>` for a non-empty accepted
secret or `no pin` otherwise. With `--json`, it MUST emit the corresponding
legacy JSON representation. Exit statuses MUST be `0`, `10`, `20`, and `1`
for OK, cancel, not-ok, and error respectively.

#### Scenario: successful secret prompt
- **WHEN** a CLI request includes `--get-pin` and the user confirms a non-empty
  secret
- **THEN** stdout contains the selected plain or JSON OK result and the secret,
  and the process exits `0`

#### Scenario: invalid CLI request
- **WHEN** a CLI request includes `--prompt` without `--get-pin`
- **THEN** it exits `1` without initializing a frontend

### Requirement: SSH askpass contract

The askpass mode MUST request a secret through the shared prompt path. Its
non-empty argument string MUST become the prompt title; an empty argument list
MUST use the legacy default title `SSH Password:`. It MUST offer `Ok` and
`Abort` labels. On a non-empty accepted secret, it MUST write only that secret
followed by a newline to stdout and exit `0`. On cancellation, not-ok, empty
secret, or error, it MUST write no secret and exit nonzero.

#### Scenario: askpass accepts a secret
- **WHEN** `nowayprompt-ssh-askpass` receives a prompt argument and the user
  confirms a non-empty secret
- **THEN** stdout is exactly the secret plus one newline and the process exits
  `0`

#### Scenario: askpass cancellation
- **WHEN** the user aborts an askpass prompt
- **THEN** stdout contains no secret and the process exits nonzero
