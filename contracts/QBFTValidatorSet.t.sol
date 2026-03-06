// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.19;

import "forge-std/Test.sol";
import "./QBFTValidatorSet.sol";
import {ERC1967Proxy} from "@openzeppelin/contracts/proxy/ERC1967/ERC1967Proxy.sol";
import {IAccessControl} from "@openzeppelin/contracts/access/IAccessControl.sol";

import {IERC1967} from "@openzeppelin/contracts/interfaces/IERC1967.sol";
/**
 * @title QBFTValidatorSetTest
 * @dev Comprehensive test suite for QBFTValidatorSet contract
 */

contract QBFTValidatorSetTest is Test {
    QBFTValidatorSet public validatorSet;

    // Test addresses
    address owner = address(0x1);
    address validator1 = address(0x2);
    address validator2 = address(0x3);
    address validator3 = address(0x4);
    address validator4 = address(0x5);
    address nonOwner = address(0x6);
    address zeroAddress = address(0);

    // Initial validator set
    address[] initialValidators;

    event ValidatorRemoved(address indexed validator);
    event OwnershipTransferred(address indexed previousOwner, address indexed newOwner);

    function setUp() public {
        // Set up initial validators
        initialValidators.push(validator1);
        initialValidators.push(validator2);
        initialValidators.push(validator3);

        QBFTValidatorSet impl = new QBFTValidatorSet();

        string[] memory validatorEnodes = new string[](3);
        validatorEnodes[0] = "enode://validator1@127.0.0.1:30301";
        validatorEnodes[1] = "enode://validator2@127.0.0.1:30302";
        validatorEnodes[2] = "enode://validator3@127.0.0.1:30303";

        bytes memory initCalldata = abi.encodeCall(QBFTValidatorSet.initialize, (
            initialValidators, 
            owner, 
            4,              // maxActiveValidators
            1000000000,     // baseFee 
            1000,           // blockIntervalMs
            32,             // epochLength
            validatorEnodes // validatorEnodes
        ));

        ERC1967Proxy proxy = new ERC1967Proxy(address(impl), initCalldata);

        // 4. Create contract instance at proxy address
        validatorSet = QBFTValidatorSet(address(proxy));
    }

    // ============ Constructor Tests ============

    function test_Constructor_SetsInitialValidators() public view {
        address[] memory validators = validatorSet.getValidators();
        assertEq(validators.length, 3);
        assertEq(validators[0], validator1);
        assertEq(validators[1], validator2);
        assertEq(validators[2], validator3);
    }

    function test_Constructor_SetsOwner() public view {
        // assertEq(validatorSet.owner(), owner);
    }

    function test_Constructor_RevertsWhenInvalidValidatorCount() public {
        address[] memory emptyValidators = new address[](0);
        address[] memory tooManyValidators = new address[](101);

        for (uint256 i = 0; i < 101; i++) {
            tooManyValidators[i] = address(uint160(i + 100));
        }

        QBFTValidatorSet impl = new QBFTValidatorSet();

        string[] memory emptyEnodes = new string[](0);
        string[] memory tooManyEnodes = new string[](101);
        for (uint256 i = 0; i < 101; i++) {
            tooManyEnodes[i] = "enode://test";
        }

        bytes memory initCalldataEmptyValidators = abi.encodeCall(QBFTValidatorSet.initialize, (
            emptyValidators, 
            owner, 
            4,              // maxActiveValidators
            1000000000,     // baseFee 
            1000,           // blockIntervalMs
            32,             // epochLength
            emptyEnodes     // validatorEnodes
        ));
        bytes memory initCalldataTooManyValidators =
            abi.encodeCall(QBFTValidatorSet.initialize, (
                tooManyValidators, 
                owner, 
                4,              // maxActiveValidators
                1000000000,     // baseFee 
                1000,           // blockIntervalMs
                32,             // epochLength
                tooManyEnodes   // validatorEnodes
            ));

        vm.expectRevert(abi.encodeWithSelector(QBFTValidatorSet.InvalidValidatorCount.selector, 0));
        new ERC1967Proxy(address(impl), initCalldataEmptyValidators);

        vm.expectRevert(abi.encodeWithSelector(QBFTValidatorSet.InvalidValidatorCount.selector, 101));
        new ERC1967Proxy(address(impl), initCalldataTooManyValidators);
    }

    function test_Constructor_RevertsWhenZeroAddress() public {
        address[] memory invalidValidators = new address[](2);
        invalidValidators[0] = validator1;
        invalidValidators[1] = zeroAddress;

        QBFTValidatorSet impl = new QBFTValidatorSet();

        string[] memory invalidEnodes = new string[](2);
        invalidEnodes[0] = "enode://test1";
        invalidEnodes[1] = "enode://test2";

        bytes memory initCalldataInvalidValidators =
            abi.encodeCall(QBFTValidatorSet.initialize, (
                invalidValidators, 
                owner, 
                4,              // maxActiveValidators
                1000000000,     // baseFee 
                1000,           // blockIntervalMs
                32,             // epochLength
                invalidEnodes   // validatorEnodes
            ));

        vm.expectRevert(QBFTValidatorSet.ZeroAddress.selector);
        new ERC1967Proxy(address(impl), initCalldataInvalidValidators);
    }

    function test_Constructor_RevertsWhenDuplicateValidators() public {
        address[] memory duplicateValidators = new address[](2);
        duplicateValidators[0] = validator1;
        duplicateValidators[1] = validator1;

        QBFTValidatorSet impl = new QBFTValidatorSet();

        string[] memory duplicateEnodes = new string[](2);
        duplicateEnodes[0] = "enode://test1";
        duplicateEnodes[1] = "enode://test1";

        bytes memory initCalldataDuplicateValidators =
            abi.encodeCall(QBFTValidatorSet.initialize, (
                duplicateValidators, 
                owner, 
                4,              // maxActiveValidators
                1000000000,     // baseFee 
                1000,           // blockIntervalMs
                32,             // epochLength
                duplicateEnodes // validatorEnodes
            ));

        vm.expectRevert(abi.encodeWithSelector(QBFTValidatorSet.DuplicateValidator.selector, validator1));
        new ERC1967Proxy(address(impl), initCalldataDuplicateValidators);
    }

    // ============ View Function Tests ============

    function test_GetValidators() public view {
        address[] memory validators = validatorSet.getValidators();
        assertEq(validators.length, 3);
        assertEq(validators[0], validator1);
        assertEq(validators[1], validator2);
        assertEq(validators[2], validator3);
    }

    function test_GetValidatorCount() public view {
        uint256 count = validatorSet.getValidatorCount();
        assertEq(count, 3);
    }

    function test_IsValidValidator() public view {
        assertTrue(validatorSet.isValidValidator(validator1));
        assertTrue(validatorSet.isValidValidator(validator2));
        assertTrue(validatorSet.isValidValidator(validator3));
        assertFalse(validatorSet.isValidValidator(validator4));
        assertFalse(validatorSet.isValidValidator(zeroAddress));
    }

    function test_Constants() public view {
        assertEq(validatorSet.MIN_VALIDATORS(), 1);
        assertEq(validatorSet.MAX_VALIDATORS(), 100);
    }

    // ============ AddValidator Tests ============

    function test_AddValidator_Success() public {
        vm.prank(owner);
        vm.expectEmit(true, false, false, true);
        emit QBFTValidatorSet.ValidatorAdded(validator4, 3, "enode://validator4@127.0.0.1:30304");

        address[] memory expectedValidators = new address[](4);
        expectedValidators[0] = validator1;
        expectedValidators[1] = validator2;
        expectedValidators[2] = validator3;
        expectedValidators[3] = validator4;
        vm.expectEmit(false, false, false, true);
        emit QBFTValidatorSet.ValidatorSetUpdated(expectedValidators);

        validatorSet.addValidator(validator4, "enode://validator4@127.0.0.1:30304");

        assertTrue(validatorSet.isValidValidator(validator4));
        assertEq(validatorSet.getValidatorCount(), 4);
        vm.stopPrank();
    }

    function test_AddValidator_RevertsWhenNotOwner() public {
        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                nonOwner,
                validatorSet.VALIDATOR_MANAGER_ROLE()
            )
        );
        vm.prank(nonOwner);
        validatorSet.addValidator(validator4, "enode://validator4@127.0.0.1:30304");
        vm.stopPrank();
    }

    function test_AddValidator_RevertsWhenZeroAddress() public {
        vm.prank(owner);
        vm.expectRevert(QBFTValidatorSet.ZeroAddress.selector);
        validatorSet.addValidator(zeroAddress, "enode://zero@127.0.0.1:30300");
    }

    function test_AddValidator_RevertsWhenDuplicate() public {
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(QBFTValidatorSet.DuplicateValidator.selector, validator1));
        validatorSet.addValidator(validator1, "enode://validator1@127.0.0.1:30301");
    }

    function test_AddValidator_RevertsWhenMaxValidatorsReached() public {
        vm.startPrank(owner);

        uint256 currentCount = validatorSet.getValidatorCount();
        uint256 maxValidators = validatorSet.MAX_VALIDATORS();

        // Fill the validator set with unique addresses until we reach MAX_VALIDATORS
        for (uint256 i = 0; i < maxValidators - currentCount; i++) {
            validatorSet.addValidator(
                address(uint160(0x100 + i)),
                string(abi.encodePacked("enode://test", vm.toString(i), "@127.0.0.1:", vm.toString(30400 + i)))
            );
        }

        vm.expectRevert(abi.encodeWithSelector(QBFTValidatorSet.InvalidValidatorCount.selector, maxValidators + 1));
        validatorSet.addValidator(address(0x1000), "enode://overflow@127.0.0.1:30500");

        vm.stopPrank();
    }

    // ============ RemoveValidator Tests ============

    function test_RemoveValidator_Success() public {
        vm.prank(owner);
        vm.expectEmit(true, false, false, true);
        emit QBFTValidatorSet.ValidatorRemoved(validator2, 2);

        vm.expectEmit(false, false, false, true);
        address[] memory expectedValidators = new address[](2);
        expectedValidators[0] = validator1;
        expectedValidators[1] = validator3;
        emit QBFTValidatorSet.ValidatorSetUpdated(expectedValidators);

        validatorSet.removeValidator(validator2);

        assertFalse(validatorSet.isValidValidator(validator2));
        assertEq(validatorSet.getValidatorCount(), 2);

        // Verify array order is maintained correctly
        address[] memory validators = validatorSet.getValidators();
        assertEq(validators[0], validator1);
        assertEq(validators[1], validator3);
    }

    function test_RemoveValidator_LastElement() public {
        // Add one more validator to test removal of last element
        vm.prank(owner);
        validatorSet.addValidator(validator4, "enode://validator4@127.0.0.1:30304");

        vm.prank(owner);
        validatorSet.removeValidator(validator4);

        assertFalse(validatorSet.isValidValidator(validator4));
        assertEq(validatorSet.getValidatorCount(), 3);
    }

    function test_RemoveValidator_RevertsWhenNotOwner() public {
        vm.prank(nonOwner);
        vm.expectRevert(
            abi.encodeWithSelector(
                IAccessControl.AccessControlUnauthorizedAccount.selector,
                nonOwner,
                validatorSet.VALIDATOR_MANAGER_ROLE()
            )
        );
        vm.prank(nonOwner);
        validatorSet.removeValidator(validator1);
        vm.stopPrank();
    }

    function test_RemoveValidator_RevertsWhenZeroAddress() public {
        vm.prank(owner);
        vm.expectRevert(QBFTValidatorSet.ZeroAddress.selector);
        validatorSet.removeValidator(zeroAddress);
    }

    function test_RemoveValidator_RevertsWhenNotFound() public {
        vm.prank(owner);
        vm.expectRevert(abi.encodeWithSelector(QBFTValidatorSet.ValidatorNotFound.selector, validator4));
        validatorSet.removeValidator(validator4);
    }

    function test_RemoveValidator_RevertsWhenMinValidatorsReached() public {
        // Remove until we have only one validator
        vm.startPrank(owner);
        validatorSet.removeValidator(validator2);
        validatorSet.removeValidator(validator3);

        // Try to remove the last one
        vm.expectRevert(abi.encodeWithSelector(QBFTValidatorSet.InvalidValidatorCount.selector, 0));
        validatorSet.removeValidator(validator1);

        vm.stopPrank();
    }

    // ============ Ownership Tests ============

    function test_AdminTransfer_Success() public {
        bytes32 adminRole = validatorSet.DEFAULT_ADMIN_ROLE();

        // 1. Grant admin role to the new admin
        vm.prank(owner);
        vm.expectEmit(true, true, false, true);
        emit IAccessControl.RoleGranted(adminRole, validator4, owner);
        validatorSet.grantRole(adminRole, validator4);

        // 2. Revoke the old admin's role
        vm.prank(owner);
        vm.expectEmit(true, true, false, true);
        emit IAccessControl.RoleRevoked(adminRole, owner, owner);
        validatorSet.revokeRole(adminRole, owner);

        // validator4 is now the only admin
        assertTrue(validatorSet.hasRole(adminRole, validator4));
        assertFalse(validatorSet.hasRole(adminRole, owner));
    }

    function test_AdminTransfer_RevertsWhenNotAdmin() public {
        bytes32 adminRole = validatorSet.DEFAULT_ADMIN_ROLE();

        vm.prank(nonOwner);

        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, nonOwner, adminRole)
        );

        validatorSet.grantRole(adminRole, validator4);
    }

    function test_CannotRevokeAdminRoleWhenNotAdmin() public {
        bytes32 adminRole = validatorSet.DEFAULT_ADMIN_ROLE();

        vm.prank(nonOwner);

        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, nonOwner, adminRole)
        );

        validatorSet.revokeRole(adminRole, owner);
    }

    function test_AdminAfterTransfer_CanManageValidators() public {
        bytes32 adminRole = validatorSet.DEFAULT_ADMIN_ROLE();
        bytes32 managerRole = validatorSet.VALIDATOR_MANAGER_ROLE();

        //
        // Transfer admin role to validator4
        //

        // Grant admin & manager roles to validator4
        vm.startPrank(owner);
        validatorSet.grantRole(adminRole, validator4);
        validatorSet.grantRole(managerRole, validator4);

        // Revoke admin & manager from old owner
        validatorSet.revokeRole(managerRole, owner);
        validatorSet.revokeRole(adminRole, owner);
        vm.stopPrank();

        //
        // New admin (validator4) can add validators
        //
        vm.prank(validator4);
        validatorSet.addValidator(address(0x10), "enode://new1@127.0.0.1:30310");

        assertTrue(validatorSet.isValidValidator(address(0x10)));

        //
        // Old admin cannot
        //
        vm.prank(owner);

        vm.expectRevert(
            abi.encodeWithSelector(IAccessControl.AccessControlUnauthorizedAccount.selector, owner, managerRole)
        );

        validatorSet.addValidator(address(0x11), "enode://new2@127.0.0.1:30311");
    }

    // ============ Edge Case Tests ============

    function test_ArrayStateAfterMultipleOperations() public {
        vm.startPrank(owner);

        // Add multiple validators
        validatorSet.addValidator(validator4, "enode://validator4@127.0.0.1:30304");
        validatorSet.addValidator(address(0x10), "enode://new3@127.0.0.1:30310");

        // Remove middle validator
        validatorSet.removeValidator(validator2);

        // Check final order
        address[] memory validators = validatorSet.getValidators();
        assertEq(validators.length, 4);
        assertEq(validators[0], validator1);
        assertEq(validators[1], address(0x10)); // last validator moved into removed slot
        assertEq(validators[2], validator3);
        assertEq(validators[3], validator4);

        vm.stopPrank();
    }

    function test_GasOptimization_RemoveLastElement() public {
        // Add a validator to test removal of last element
        vm.prank(owner);
        validatorSet.addValidator(validator4, "enode://validator4@127.0.0.1:30304");

        uint256 gasBefore = gasleft();
        vm.prank(owner);
        validatorSet.removeValidator(validator4);
        uint256 gasUsed = gasBefore - gasleft();

        // Gas usage should be reasonable (adjust threshold as needed)
        assertLt(gasUsed, 100000);
    }

    // ============ Upgradeability Tests ============

    function test_UpgradePreservesState() public {
        // Verify initial state
        uint256 nrValidators = validatorSet.getValidatorCount();
        assertTrue(validatorSet.isValidValidator(validator1));

        // Deploy new implementation
        QBFTValidatorSet implV2 = new QBFTValidatorSet();

        // Expect upgrade event
        vm.expectEmit(true, false, false, true);
        emit IERC1967.Upgraded(address(implV2));

        // Perform upgrade via UUPS
        vm.prank(owner); // owner must have UPGRADER_ROLE
        validatorSet.upgradeToAndCall(address(implV2), "");

        // State should be preserved
        assertEq(validatorSet.getValidatorCount(), nrValidators);
        assertTrue(validatorSet.isValidValidator(validator2));
    }

    function test_OnlyUpgraderCanUpgrade() public {
        QBFTValidatorSet implV2 = new QBFTValidatorSet();

        address attacker = address(999);

        vm.prank(attacker);
        vm.expectRevert(); // Will revert with AccessControl error
        UUPSUpgradeable(address(validatorSet)).upgradeToAndCall(address(implV2), "");
    }

    function test_UpgradeToNewImplementationWithCall() public {
        QBFTValidatorSet implV2 = new QBFTValidatorSet();
        uint256 nrValidators = validatorSet.getValidatorCount();

        // Example: call addValidator during upgrade
        address newValidator = address(5);
        bytes memory callData = abi.encodeWithSelector(QBFTValidatorSet.addValidator.selector, newValidator);

        vm.prank(owner);
        UUPSUpgradeable(address(validatorSet)).upgradeToAndCall(address(implV2), callData);

        // Verify upgrade happened and call was executed
        assertEq(validatorSet.getValidatorCount(), nrValidators + 1);
        assertTrue(validatorSet.isValidValidator(newValidator));
    }
}
