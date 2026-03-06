# Validator Inspector Automation DSL

This directory hosts a small Solidity-based DSL that lets you orchestrate
validator actions from a script. Scripts are regular Solidity contracts that
extend `ValidatorInspectorScript` and push commands such as launching the chain,
starting or stopping validators, spamming transactions, or scheduling health
checks.

## Project layout

- `contracts/dsl` contains the reusable building blocks:
  - `InspectorCommands.sol` defines the ABI shared with the Rust automation
    runtime.
  - `ValidatorInspectorScript.sol` exposes helper methods such as `at(...)`,
    `after(...)`, `every(...)`, and the different `schedule*` helpers.
- `contracts/scripts` hosts example scripts. You can add custom plans here.
- `foundry.toml` configures a local Foundry project that compiles the DSL.

## Writing a script

```solidity
import "../dsl/ValidatorInspectorScript.sol";

contract ScaleUpPlan is ValidatorInspectorScript {
    function build() internal override {
        scheduleLaunch(
            at(0),
            InspectorCommands.LaunchChainArgs({
                totalValidators: 4,
                basePort: 8545
            })
        );

        scheduleStart(afterDelay(5), "v0");
        scheduleLaunchValidator(afterDelay(10), true);
        scheduleSpam(
            every(afterDelay(10), 30, 5),
            InspectorCommands.SpamArgs({
                totalTxs: 50,
                parallel: 2,
                burst: 10,
                accounts: 5,
                mode: "round-robin",
                target: "",
                hasTarget: false
            })
        );
    }
}
```

Key helpers:

- `at(seconds)` schedules a command at an absolute offset from script start.
- `afterDelay(seconds)` advances the internal cursor, making it easy to build
  sequential flows.
- `every(timing, interval, repeats)` converts a timing into a recurring task.
- `scheduleLaunch`, `scheduleStart`, `scheduleStop`, `scheduleRestart`,
  `scheduleLaunchValidator`, `schedulePing`, and `scheduleSpam` enqueue the
  supported commands.

All timestamps are expressed in seconds. `repeatCount` represents how many
additional executions should run after the first one.

## Using automation in the TUI

Run the inspector as usual (pointing `--automation-project` at this directory if
you’re outside the repo). Inside the UI press `a` to open the Automation Scripts
modal, pick a contract (e.g., `ExamplePlan`), and the inspector will:

- Run `forge build --force` to (re)compile your scripts.
- Deploy the selected contract in an embedded EVM and read `commands()`.
- Stream script and runtime logs into the Automation panel (bottom-right), which
  auto-scrolls as new lines arrive. The panel also shows the current script
  label and a hint that `[x]` stops the running script.
