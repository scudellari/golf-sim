# golf-sim

A deterministic golf simulation in Rust. It covers ball flight, the strike, bounce and roll on real terrain, the day's weather and turf, the Rules of Golf, and the flow of a hole and a round.

The same seed plays the same round on every platform. An AI agent can play a full round through the library or the command-line program, and a game engine can call the same core to draw the round and handle input for human players.

The core is being built in phases. Version 0.2.2 plays a stroke and a hole on a constant day. [CHANGELOG.md](CHANGELOG.md) says what is in each release.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

The measured data in `testdata/validation/targets/` is not covered by these licences. Each file keeps the terms of its source, which [`LICENCES.toml`](testdata/validation/targets/LICENCES.toml) lists with the attribution each source asks for. The Biber et al. (2023) turf-bounce data is used under CC BY 4.0.

See [Contributing](CONTRIBUTING.md#validation-data-and-fixtures) for validation and fixture setup.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
