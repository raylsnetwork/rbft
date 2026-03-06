// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.25;

import "./InspectorCommands.sol";

abstract contract ValidatorInspectorScript {
    using InspectorCommands for InspectorCommands.Timing;

    InspectorCommands.Command[] internal _commands;
    uint64 internal _cursor;

    constructor() {
        build();
    }

    function build() internal virtual;

    function commands() external view returns (InspectorCommands.Command[] memory) {
        return _commands;
    }

    function resetSequence() internal {
        _cursor = 0;
    }

    function at(uint64 offsetSeconds) internal pure returns (InspectorCommands.Timing memory timing) {
        timing.offsetSeconds = offsetSeconds;
    }

    function afterDelay(uint64 deltaSeconds) internal returns (InspectorCommands.Timing memory timing) {
        _cursor += deltaSeconds;
        timing.offsetSeconds = _cursor;
    }

    function every(
        InspectorCommands.Timing memory timing,
        uint64 intervalSeconds,
        uint32 repeatCount
    ) internal pure returns (InspectorCommands.Timing memory) {
        timing.intervalSeconds = intervalSeconds;
        timing.repeatCount = repeatCount;
        return timing;
    }

    function scheduleLaunch(
        InspectorCommands.Timing memory timing,
        InspectorCommands.LaunchChainArgs memory args
    ) internal {
        _push(
            InspectorCommands.CommandKind.LaunchChain,
            timing,
            abi.encode(args)
        );
    }

    function scheduleStart(
        InspectorCommands.Timing memory timing,
        string memory label
    ) internal {
        InspectorCommands.LabelArgs memory args = InspectorCommands.LabelArgs({label: label});
        _push(
            InspectorCommands.CommandKind.StartValidator,
            timing,
            abi.encode(args)
        );
    }

    function scheduleStop(
        InspectorCommands.Timing memory timing,
        string memory label
    ) internal {
        InspectorCommands.LabelArgs memory args = InspectorCommands.LabelArgs({label: label});
        _push(
            InspectorCommands.CommandKind.StopValidator,
            timing,
            abi.encode(args)
        );
    }

    function scheduleRestart(
        InspectorCommands.Timing memory timing,
        string memory label
    ) internal {
        InspectorCommands.LabelArgs memory args = InspectorCommands.LabelArgs({label: label});
        _push(
            InspectorCommands.CommandKind.RestartValidator,
            timing,
            abi.encode(args)
        );
    }

    function schedulePing(
        InspectorCommands.Timing memory timing,
        string memory label
    ) internal {
        InspectorCommands.LabelArgs memory args = InspectorCommands.LabelArgs({label: label});
        _push(
            InspectorCommands.CommandKind.PingValidator,
            timing,
            abi.encode(args)
        );
    }

    function scheduleLaunchValidator(
        InspectorCommands.Timing memory timing,
        bool autoStart
    ) internal {
        InspectorCommands.LaunchValidatorArgs memory args =
            InspectorCommands.LaunchValidatorArgs({autoStart: autoStart});
        _push(
            InspectorCommands.CommandKind.LaunchValidator,
            timing,
            abi.encode(args)
        );
    }

    function scheduleSpam(
        InspectorCommands.Timing memory timing,
        InspectorCommands.SpamArgs memory args
    ) internal {
        _push(
            InspectorCommands.CommandKind.SpamJob,
            timing,
            abi.encode(args)
        );
    }

    function logMessage(
        InspectorCommands.Timing memory timing,
        string memory message
    ) internal {
        InspectorCommands.LogArgs memory args = InspectorCommands.LogArgs({message: message});
        _push(
            InspectorCommands.CommandKind.LogMessage,
            timing,
            abi.encode(args)
        );
    }

    function _push(
        InspectorCommands.CommandKind kind,
        InspectorCommands.Timing memory timing,
        bytes memory payload
    ) internal {
        _commands.push(
            InspectorCommands.Command({
                offsetSeconds: timing.offsetSeconds,
                intervalSeconds: timing.intervalSeconds,
                repeatCount: timing.repeatCount,
                kind: kind,
                payload: payload
            })
        );
    }
}
