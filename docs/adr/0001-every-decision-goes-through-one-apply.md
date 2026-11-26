# 0001. Every decision goes through one `apply`, for people, agents and replays

- Status: Accepted. Amended by [0005](0005-tests-and-agents-state-a-swing-or-use-a-seeded-profile.md): a test or an agent may state a swing exactly. [0004](0004-checked-against-measured-golf-not-the-earlier-implementation-s-bits.md) changes how the console and the records are checked against the reference.
- Date: 2026-11-26

## Context

Three callers play the same round: a person through a game engine, an agent through the library or the `golf` program, and a replay of a saved record. A person's input is key presses on a clock. In the reference implementation, a relief choice is a cursor step and a confirm key, and the session's phase is derived from screen state. An agent needs decisions, not keystrokes. A replay must reach the same result as live play, exactly, or refuse.

## Decision

- A decision (a stroke, a drop, a choice, a placement, a provisional ball, going forward, picking up) is an `Action`. `Hole::act` resolves an action into a record entry, and one function, `apply(entry)`, applies every entry. Live play stores the results on the entry. A replay compares them and refuses at the first difference.
- The prompt is derived from the rules state on every call. Observation, legal actions and previews take `&self` and change nothing.
- A person's input goes through the `Console`, which owns the screen's state (the address being built, the meter, the relief cursor, the handed club, playback) and produces the same actions.
- An agent submits a swing as an intent. A `Skill` presses a real meter with seeded errors, and the meter's judgment builds the `Execution`. No public type accepts an `Execution` from a caller.

## Alternatives considered

- **The agent sends the console's key presses.** Every caller would learn the screen's state machine, its selection indices and its timing, and the interface would hide nothing.
- **The agent submits the execution directly.** It skips the timing window that a hard lie narrows, so an agent would play every lie as a perfect swing.
- **A separate fast engine for agents.** Two ways to play a hole, and an agent's results would not be the game's.

## Consequences

- An agent's record is a person's record. A game can replay a round an agent played, and the reverse.
- The record is the proof: the core replays its own records bit for bit, and plays the reference implementation's recorded decisions and compares the results within a tolerance (ADR 0004).
- The console carries screen state that the reference kept inside its session. It is checked against the reference's session stamp by stamp, discrete fields exactly and numbers within a tolerance (ADR 0004).
