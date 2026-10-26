// SPDX-License-Identifier: MIT
pragma solidity 0.8.35;

/// @dev Only the entry type that `MaliciousRejectList` returns. Full interface: rgb-lib.
interface IRgbRejectList {
    struct Entry {
        bytes32 opId;
        uint16 assignmentType;
        uint16 no;
        bool reject;
    }
}
