# 0005. Tests and agents state a swing exactly or use a seeded skill profile

- Status: Accepted
- Date: 2026-11-26

## Context

[ADR 0001](0001-every-decision-goes-through-one-apply.md) treated an agent as a player who competes with people. It let an agent submit only an intent, which a skill model pressed on the real meter, so no caller could hand the core an `Execution`. The first plan also let an agent wait at address for a gust and limited what it could read.

The owner then set the agent's purpose on 2026-11-26: automated testing, playtesting and balance testing. "Shouldn't the tests largely be deterministic with expected results?" Competition between agents and people may come later, but not now.

## Decision

- A stroke's swing is one of two kinds, and both go through the same `apply` as a person's presses and a replay:
  - **Exact.** `Swing::Exact { power, timing_fault, strike_m }` states the meter's result, and the core fills in the rest from the swing spec. Regression tests use it.
  - **A skill profile.** `Swing::Skill(profile)` presses a real swing session with seeded errors. Balance sweeps use it, and each seed gives one result.
- Each record entry says which kind made its execution, with the profile's values, and the record holds the seed.
- An agent's swing starts a fixed 0.25 s after the address starts. No caller waits at address.
- The interface limits no reader. The scripted golfer decides from what a player would know, as a choice inside that golfer.
- `golf batch`, `golf sweep` and `golf scenario` serve balance sweeps and expected-result tests.

## Alternatives considered

- **Only a skill model on the meter.** Every regression test would depend on simulated hands, and a test author could not state the swing a bug needs.
- **An agent sends the console's key presses.** Every test would learn the screen's state machine and its timing.

## Consequences

- A test states a swing and expects one outcome. A sweep states a profile and a seed range and gets one distribution.
- A record made by a test replays like a person's record, and a game can show it.
- The competition parts are deferred, not blocked. `observe` is one function that a later information filter can wrap, and the record's `driver` and `swing` dictionaries keep unknown keys. The design doc's last section lists what competition would add.
