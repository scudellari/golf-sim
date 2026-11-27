# Debug a replay or a Godot process

Run the shell commands from the workspace root. The `debugging` profile keeps release optimizations and includes full debug information. The compiler can inline calls and remove local variables.

## Replay in LLDB

Build `golf` with symbols and open the lip-out bundle in LLDB.

```sh
just debug-replay testdata/bundles/lip-out
```

The recipe builds `target/debugging/golf` and sets its arguments to `replay testdata/bundles/lip-out`. At the LLDB prompt, set a breakpoint on the first statement in `golf_sim_physics::bounce::resolve_impact`. Run the replay and print the stack.

```text
breakpoint set --file bounce.rs --line 193
run
thread backtrace
```

The breakpoint stops on the approach shot, before the putt that lips out. Look for `resolve_impact` at `bounce.rs` in the backtrace. Enter `breakpoint disable 1` and `continue` to finish the replay. The program prints `Stroke 2 replay matched bit for bit.`

The batch check uses the same recipe and requires a breakpoint stop with a source line.

```sh
sh scripts/check-debug-replay.sh
```

## Replay in VS Code

1. Open the workspace root in VS Code and install the **CodeLLDB** extension.
2. Open [`bounce.rs`](../../crates/golf-sim-physics/src/bounce.rs) and click beside `let _span = log::bounce().entered();` in `resolve_impact`.
3. In **Run and Debug**, select **golf replay** and start debugging. CodeLLDB builds `golf` with the `debugging` profile and runs the lip-out bundle.
4. When the debugger stops in `resolve_impact`, use **Call Stack** to see which functions called it.

To replay another bundle, change the bundle path in `args` in `.vscode/launch.json`.

## Attach to a running Godot game

Run these steps once `golf-sim-godot` has a GDExtension entry point and a Godot project loads it. Build the binding with symbols.

```sh
cargo build --offline -p golf-sim-godot --profile debugging
```

1. The library in `target/debugging/` is `libgolf_sim_godot.dylib` on macOS, `libgolf_sim_godot.so` on Linux, or `golf_sim_godot.dll` on Windows. Put it in the Godot project. Set the matching debug entry in the `.gdextension` file's `[libraries]` section to its `res://` path. On macOS, keep any `.dSYM` bundle beside the library.
2. Start the game and find the PID of the game process. If you launched the game from the editor, select the game process.
3. Replace `PID` with that process ID and attach.

   ```sh
   lldb -p PID
   ```

4. At the LLDB prompt, use `image list -f` to confirm Godot loaded the library you built. Set a breakpoint with `breakpoint set --file bounce.rs --line 193`. Enter `continue` and play a shot that bounces. Enter `thread backtrace` at the stop and `process detach` when you want to leave the game running.

In VS Code, select **Attach to Godot** and pick the same process. Set the breakpoint in `bounce.rs` and continue the game.
