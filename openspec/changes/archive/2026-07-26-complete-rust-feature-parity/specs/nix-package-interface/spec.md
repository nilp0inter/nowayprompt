## ADDED Requirements

### Requirement: Public executable installation

The Nix package MUST install one compiled Rust executable as
`$out/bin/nowayprompt` and expose `$out/bin/pinentry-nowayprompt` and
`$out/bin/nowayprompt-ssh-askpass` as aliases that preserve their invocation
basename. The package's main program MUST be `nowayprompt`.

#### Scenario: aliases select their contracts
- **WHEN** each installed executable path is invoked
- **THEN** the base path selects CLI mode, the pinentry path selects Assuan
  mode, and the askpass path selects askpass mode

### Requirement: Rust-owned manual pages

The Nix package MUST install manual pages for `nowayprompt`,
`pinentry-nowayprompt`, `nowayprompt-ssh-askpass`, and the shared configuration
format. Documentation MUST describe the installed Rust names and their public
contracts; it MUST NOT direct users to the legacy `wayprompt` executables.

#### Scenario: installed manuals are discoverable
- **WHEN** the package output is inspected
- **THEN** its manpage directories contain pages for each installed entrypoint
  and the configuration format
