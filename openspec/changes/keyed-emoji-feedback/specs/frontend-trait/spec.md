## MODIFIED Requirements

### Requirement: Frontend trait interface
The module MUST define a `Frontend` trait that both the TTY fallback and the Wayland frontend implement. The trait MUST expose: `init(&mut self, cfg: &mut Config) -> Result<RawFd, FrontendError>` (opens the frontend, returns the poll-able fd); `deinit(&mut self)` (restores terminal/closes resources); `enter_mode(&mut self, mode: InterfaceMode) -> Result<(), FrontendError>` (enters a UI mode); `handle_event(&mut self) -> Result<Event, FrontendError>` (blocks for or drains an event); `flush(&mut self) -> Result<Option<Event>, FrontendError>` (drains pending non-blocking events; returns `Ok(None)` if none); `no_event(&mut self) -> Result<(), FrontendError>` (called when `poll` returned no frontend event but another descriptor may have been ready); `next_deadline(&self) -> Option<std::time::Instant>` (returns the next armed monotonic frontend deadline); and `handle_timeout(&mut self) -> Result<(), FrontendError>` (processes a due frontend deadline). Both implementations MUST share exactly these eight methods.

#### Scenario: Frontend returns a pollable fd
- **WHEN** `Frontend::init` succeeds
- **THEN** it returns a `RawFd` that can be registered with `poll(2)` alongside stdin

#### Scenario: enter_mode transitions the UI
- **WHEN** `enter_mode(GetPin)` is called on an idle frontend
- **THEN** the frontend renders the pin-entry UI and is ready to accept input events

#### Scenario: Frontend exposes no inactive deadline
- **WHEN** feedback is disabled, below its minimum, or outside `GetPin` mode
- **THEN** `next_deadline()` returns `None`

#### Scenario: Wayland frontend implements Frontend
- **WHEN** the `Wayland` frontend struct is defined
- **THEN** it implements all eight `Frontend` trait methods and compiles against the updated trait

### Requirement: InterfaceMode enum

The module MUST define an `InterfaceMode` enum with variants `None`, `GetPin`, `Message`. `None` is the idle state. `GetPin` renders the pin-entry feedback row, using the configured fixed-length mask whenever emoji feedback is enabled but no signature is revealed, and legacy squares when emoji feedback is off. `Message` renders the confirm/message display. The `confirm` and `message` Assuan modes collapse into the same frontend `Message` mode, matching the pinned `pkgs.wayprompt` oracle.

#### Scenario: GetPin mode renders pin input
- **WHEN** `enter_mode(GetPin)` is called
- **THEN** the frontend renders the ` > ` prompt with the configured mask row when emoji feedback is enabled, otherwise legacy square feedback

#### Scenario: Message mode renders display-only
- **WHEN** `enter_mode(Message)` is called
- **THEN** the frontend renders title/description/error without a pin input row or emoji deadline

## ADDED Requirements

### Requirement: Monotonic frontend deadline semantics
`next_deadline` MUST return an absolute monotonic `Instant`, and `handle_timeout` MUST be idempotent when no deadline is due. A timeout handler MUST process only frontend timing state; it MUST NOT synthesize Enter, OK, cancellation, or other user events.

#### Scenario: Due timeout updates display only
- **WHEN** `handle_timeout` processes an armed idle-feedback deadline
- **THEN** it updates and renders feedback but emits no `Event::UserOk`

#### Scenario: Early timeout call is harmless
- **WHEN** `handle_timeout` is called before the returned deadline
- **THEN** it leaves the feedback state unchanged

### Requirement: Deadline state resets at mode boundaries
Entering `GetPin` MUST start with no inherited deadline, signature, or timing samples. Entering `None` or `Message`, terminal user events, and `deinit` MUST disarm the deadline and clear prompt-local feedback state.

#### Scenario: Prompt cannot inherit prior signature
- **WHEN** one prompt ends with emoji feedback visible and another prompt enters `GetPin`
- **THEN** the new prompt starts with its initial mask or legacy square row and empty deadline/sample state
