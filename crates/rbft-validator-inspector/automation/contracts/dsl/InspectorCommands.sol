// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.25;

library InspectorCommands {
    enum CommandKind {
        LaunchChain,
        StartValidator,
        StopValidator,
        RestartValidator,
        LaunchValidator,
        SpamJob,
        PingValidator,
        LogMessage
    }

    struct Command {
        uint64 offsetSeconds;
        uint64 intervalSeconds;
        uint32 repeatCount;
        CommandKind kind;
        bytes payload;
    }

    struct Timing {
        uint64 offsetSeconds;
        uint64 intervalSeconds;
        uint32 repeatCount;
    }

    struct LaunchChainArgs {
        uint32 totalValidators;
        uint16 basePort;
    }

    struct LabelArgs {
        string label;
    }

    struct LaunchValidatorArgs {
        bool autoStart;
    }

    struct SpamArgs {
        uint64 totalTxs;
        uint64 parallel;
        uint64 burst;
        uint32 accounts;
        string mode;
        string target;
        bool hasTarget;
    }

    struct LogArgs {
        string message;
    }
}
