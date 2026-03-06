// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.25;

import "../dsl/ValidatorInspectorScript.sol";

contract ExamplePlan is ValidatorInspectorScript {
    function build() internal override {
        logMessage(at(0), "Starting automation plan");
        scheduleLaunch(
            at(0),
            InspectorCommands.LaunchChainArgs({
                totalValidators: 4,
                basePort: 8545
            })
        );

        scheduleStart(afterDelay(5), "v4");

        logMessage(afterDelay(5), "Scaling up validator v4 soon");

        scheduleSpam(
            every(afterDelay(10), 10, 2),
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

        scheduleLaunchValidator(afterDelay(10), true);
        logMessage(afterDelay(2), "Validator v4 requested");
        schedulePing(afterDelay(5), "v4");
        scheduleStop(afterDelay(15), "v1");
        scheduleRestart(afterDelay(10), "v1");
        logMessage(afterDelay(5), "Automation plan complete");
    }
}
