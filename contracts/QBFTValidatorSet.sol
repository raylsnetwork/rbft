// SPDX-License-Identifier: Apache-2.0
pragma solidity ^0.8.19;

import "@openzeppelin/contracts-upgradeable/proxy/utils/Initializable.sol";
import "@openzeppelin/contracts-upgradeable/proxy/utils/UUPSUpgradeable.sol";
import "@openzeppelin/contracts-upgradeable/access/AccessControlUpgradeable.sol";


// Note (AT):
//
// What we could do is make this an ERC20 contract where more tokens staked
// gives a higher probability of being selected.
//
// Use swaps to buy staked tokens.
//
// At the end of each epoch run a function called
//    validatorsForNextEpoch(uint randomSeed, uint maxValidators) 
//       external view returns (uint[] memory)
//
// This function returns a bitset for the validators array picking up to maxValidators from the
// total set by sampling. It can take some time to run as it does not run on-chain and consumes no gas.
//
// find the lowest upicked validator where sum(balance[i]) >= f(randomSeed) % totalSupply
// or similar



/**
 * @title QBFTValidatorSet
 * @dev Genesis validator set contract for QBFT consensus with UUPS upgradeability
 * @notice Designed for genesis deployment at predefined address via proxy
 * @custom:genesis-proxy-address 0x0000000000000000000000000000000000001001
 */
contract QBFTValidatorSet is Initializable, UUPSUpgradeable, AccessControlUpgradeable {
    // Role definitions
    bytes32 public constant VALIDATOR_MANAGER_ROLE = keccak256("VALIDATOR_MANAGER_ROLE");
    bytes32 public constant UPGRADER_ROLE = keccak256("UPGRADER_ROLE");

    // Slot 0
    address[] private _validatorList;

    // Slot 1
    uint256 private _maxActiveValidators;

    // Slot 2
    uint256 private _baseFee;

    // Slot 3
    uint256 private _blockIntervalMs;

    // Slot 4
    uint256 private _epochLength;

    // Slot 5
    string[] private _validatorEnodes;

    mapping(address => uint256) private _validatorIndex; // 1-indexed (0 means not exists)
    mapping(address => bool) private _isValidator;

    // Constants
    uint256 public constant MIN_VALIDATORS = 1;
    uint256 public constant MAX_VALIDATORS = 100;

    // Events
    event ValidatorAdded(address indexed validator, uint256 indexed index, string enode);
    event ValidatorRemoved(address indexed validator, uint256 indexed oldIndex);
    event ValidatorSetUpdated(address[] validators);
    event MaxActiveValidatorsUpdated(uint256 oldValue, uint256 newValue);
    event BaseFeeUpdated(uint256 oldValue, uint256 newValue);
    event BlockIntervalMsUpdated(uint256 oldValue, uint256 newValue);
    event EpochLengthUpdated(uint256 oldValue, uint256 newValue);

    // Errors
    error DuplicateValidator(address validator);
    error ValidatorNotFound(address validator);
    error InvalidValidatorCount(uint256 count);
    error ZeroAddress();

    /// @custom:oz-upgrades-unsafe-allow constructor
    constructor() {
        _disableInitializers();
    }

    /**
     * @dev Initializer for genesis deployment through proxy
     * @param initialValidators Array of initial validator addresses
     * @param admin Address to receive all roles
     * @param maxActiveValidators Maximum number of active validators
     * @param baseFee Base fee per transaction
     * @param blockIntervalMs Block interval in milliseconds
     * @param epochLength Epoch length in blocks
     * @param initialEnodes Array of enode URLs for validators
     */
    function initialize(
        address[] memory initialValidators, 
        address admin,
        uint256 maxActiveValidators,
        uint256 baseFee,
        uint256 blockIntervalMs,
        uint256 epochLength,
        string[] memory initialEnodes
    ) public initializer {
        if (admin == address(0)) revert ZeroAddress();

        __AccessControl_init();

        // Setup roles
        _grantRole(DEFAULT_ADMIN_ROLE, admin);
        _grantRole(VALIDATOR_MANAGER_ROLE, admin);
        _grantRole(UPGRADER_ROLE, admin);

        // Initialize configuration values from parameters
        _maxActiveValidators = maxActiveValidators;
        _baseFee = baseFee;
        _blockIntervalMs = blockIntervalMs;
        _epochLength = epochLength;

        // Set initial validators
        _setValidators(initialValidators);

        // Set initial enodes
        _validatorEnodes = initialEnodes;
    }

    /**
     * @dev Returns the current validator set
     */
    function getValidators() external view returns (address[] memory) {
        return _validatorList;
    }

    /**
     * @dev Returns the number of validators
     */
    function getValidatorCount() external view returns (uint256) {
        return _validatorList.length;
    }

    /**
     * @dev Checks if an address is a validator (O(1) operation)
     */
    function isValidValidator(address validator) external view returns (bool) {
        return _isValidator[validator];
    }

    /**
     * @dev Returns the index of a validator in the list
     */
    function getValidatorIndex(address validator) external view returns (uint256) {
        if (!_isValidator[validator]) revert ValidatorNotFound(validator);
        return _validatorIndex[validator] - 1; // Convert to 0-indexed
    }

    /**
     * @dev Returns the maximum active validators
     */
    function getMaxActiveValidators() external view returns (uint256) {
        return _maxActiveValidators;
    }

    /**
     * @dev Returns the base fee
     */
    function getBaseFee() external view returns (uint256) {
        return _baseFee;
    }

    /**
     * @dev Sets the maximum active validators
     * @param newMaxActiveValidators The new maximum active validators value
     */
    function setMaxActiveValidators(uint256 newMaxActiveValidators) external onlyRole(VALIDATOR_MANAGER_ROLE) {
        require(newMaxActiveValidators >= 4, "min 4 active validators");
        uint256 oldValue = _maxActiveValidators;
        _maxActiveValidators = newMaxActiveValidators;
        emit MaxActiveValidatorsUpdated(oldValue, newMaxActiveValidators);
    }

    /**
     * @dev Sets the base fee
     * @param newBaseFee The new base fee value
     */
    function setBaseFee(uint256 newBaseFee) external onlyRole(VALIDATOR_MANAGER_ROLE) {
        // Minimum base fee: 0.01 RLS per 21000 gas (minimum tx)
        uint256 minBaseFee = uint256(1e16) / uint256(21000);
        // Maximum base fee: 1 RLS per 21000 gas (minimum tx)
        uint256 maxBaseFee = uint256(1e18) / uint256(21000);
        require(newBaseFee >= minBaseFee, "base fee too low");
        require(newBaseFee <= maxBaseFee, "base fee too high");
        uint256 oldValue = _baseFee;
        _baseFee = newBaseFee;
        emit BaseFeeUpdated(oldValue, newBaseFee);
    }

    /**
     * @dev Returns the block interval in milliseconds
     */
    function getBlockIntervalMs() external view returns (uint256) {
        return _blockIntervalMs;
    }

    /**
     * @dev Sets the block interval in milliseconds
     * @param newBlockIntervalMs The new block interval value in milliseconds
     */
    function setBlockIntervalMs(uint256 newBlockIntervalMs) external onlyRole(VALIDATOR_MANAGER_ROLE) {
        require(newBlockIntervalMs >= 100 && newBlockIntervalMs <= 20000,
            "block interval must be 100-20000 ms");
        uint256 oldValue = _blockIntervalMs;
        _blockIntervalMs = newBlockIntervalMs;
        emit BlockIntervalMsUpdated(oldValue, newBlockIntervalMs);
    }

    /**
     * @dev Returns the epoch length
     */
    function getEpochLength() external view returns (uint256) {
        return _epochLength;
    }

    /**
     * @dev Sets the epoch length
     * @param newEpochLength The new epoch length value
     */
    function setEpochLength(uint256 newEpochLength) external onlyRole(VALIDATOR_MANAGER_ROLE) {
        require(newEpochLength >= 4 && newEpochLength <= 128,
            "epoch length must be 4-128 blocks");
        uint256 oldValue = _epochLength;
        _epochLength = newEpochLength;
        emit EpochLengthUpdated(oldValue, newEpochLength);
    }

    /**
     * @dev Returns the validator enodes array
     */
    function getValidatorEnodes() external view returns (string[] memory) {
        return _validatorEnodes;
    }

    /**
     * @dev Returns a specific validator enode by index
     */
    function getValidatorEnode(uint256 index) external view returns (string memory) {
        require(index < _validatorEnodes.length, "index out of bounds");
        return _validatorEnodes[index];
    }

    /**
     * @dev Adds a new validator to the set (O(1) operation)
     * @param validator The validator address to add
     * @param enode The enode URL for the validator
     */
    function addValidator(address validator, string memory enode) external onlyRole(VALIDATOR_MANAGER_ROLE) {
        if (validator == address(0)) revert ZeroAddress();
        if (_isValidator[validator]) revert DuplicateValidator(validator);
        if (_validatorList.length >= MAX_VALIDATORS) {
            revert InvalidValidatorCount(_validatorList.length + 1);
        }
        require(bytes(enode).length > 0, "enode cannot be empty");

        _validatorList.push(validator);
        _validatorEnodes.push(enode);
        _validatorIndex[validator] = _validatorList.length; // 1-indexed
        _isValidator[validator] = true;

        emit ValidatorAdded(validator, _validatorList.length - 1, enode);
        emit ValidatorSetUpdated(_validatorList);
    }

    /**
     * @dev Removes a validator from the set (O(1) operation)
     */
    function removeValidator(address validator) external onlyRole(VALIDATOR_MANAGER_ROLE) {
        if (validator == address(0)) revert ZeroAddress();
        if (!_isValidator[validator]) revert ValidatorNotFound(validator);
        if (_validatorList.length <= MIN_VALIDATORS) {
            revert InvalidValidatorCount(_validatorList.length - 1);
        }

        uint256 index = _validatorIndex[validator] - 1;
        uint256 lastIndex = _validatorList.length - 1;

        if (index != lastIndex) {
            address lastValidator = _validatorList[lastIndex];
            _validatorList[index] = lastValidator;
            _validatorIndex[lastValidator] = index + 1;
            
            // Also swap enodes
            _validatorEnodes[index] = _validatorEnodes[lastIndex];
        }

        _validatorList.pop();
        _validatorEnodes.pop();
        delete _validatorIndex[validator];
        delete _isValidator[validator];

        emit ValidatorRemoved(validator, index);
        emit ValidatorSetUpdated(_validatorList);
    }

    /**
     * @dev Adds multiple validators in batch
     * @param validators Array of validator addresses to add
     * @param enodes Array of enode URLs corresponding to validators
     */
    function addValidators(
        address[] memory validators,
        string[] memory enodes
    ) external onlyRole(VALIDATOR_MANAGER_ROLE) {
        require(validators.length == enodes.length, "validators and enodes length mismatch");
        uint256 newLength = _validatorList.length + validators.length;
        if (newLength > MAX_VALIDATORS) {
            revert InvalidValidatorCount(newLength);
        }

        for (uint256 i = 0; i < validators.length; i++) {
            address validator = validators[i];
            if (validator == address(0)) revert ZeroAddress();
            if (_isValidator[validator]) revert DuplicateValidator(validator);
            require(bytes(enodes[i]).length > 0, "enode cannot be empty");

            _validatorList.push(validator);
            _validatorEnodes.push(enodes[i]);
            _validatorIndex[validator] = _validatorList.length;
            _isValidator[validator] = true;

            emit ValidatorAdded(validator, _validatorList.length - 1, enodes[i]);
        }

        emit ValidatorSetUpdated(_validatorList);
    }

    /**
     * @dev Removes multiple validators in batch
     */
    function removeValidators(address[] memory validators) external onlyRole(VALIDATOR_MANAGER_ROLE) {
        uint256 newLength = _validatorList.length - validators.length;
        if (newLength < MIN_VALIDATORS) {
            revert InvalidValidatorCount(newLength);
        }

        for (uint256 i = 0; i < validators.length; i++) {
            address validator = validators[i];
            if (!_isValidator[validator]) revert ValidatorNotFound(validator);

            uint256 index = _validatorIndex[validator] - 1;
            uint256 lastIndex = _validatorList.length - 1;

            if (index != lastIndex) {
                address lastValidator = _validatorList[lastIndex];
                _validatorList[index] = lastValidator;
                _validatorIndex[lastValidator] = index + 1;
                
                // Also swap enodes
                _validatorEnodes[index] = _validatorEnodes[lastIndex];
            }

            _validatorList.pop();
            _validatorEnodes.pop();
            delete _validatorIndex[validator];
            delete _isValidator[validator];

            emit ValidatorRemoved(validator, index);
        }

        emit ValidatorSetUpdated(_validatorList);
    }

    /**
     * @dev Internal function to set validators with validation
     */
    function _setValidators(address[] memory newValidators) internal {
        if (newValidators.length < MIN_VALIDATORS || newValidators.length > MAX_VALIDATORS) {
            revert InvalidValidatorCount(newValidators.length);
        }

        for (uint256 i = 0; i < newValidators.length; i++) {
            address validator = newValidators[i];
            if (validator == address(0)) revert ZeroAddress();
            if (_isValidator[validator]) revert DuplicateValidator(validator);

            _validatorList.push(validator);
            _validatorIndex[validator] = i + 1; // 1-indexed
            _isValidator[validator] = true;
        }

        emit ValidatorSetUpdated(_validatorList);
    }

    /**
     * @dev Required by UUPS - authorizes upgrades
     */
    function _authorizeUpgrade(address newImplementation) internal override onlyRole(UPGRADER_ROLE) {}

    /**
     * @dev Returns the version of the contract
     */
    function version() external pure returns (string memory) {
        return "1.0.0";
    }
}

/**
 * @title MinimalUUPSProxy
 * @dev Minimal UUPS proxy for genesis deployment
 * @notice This proxy will be deployed at the predefined genesis address
 */
contract MinimalUUPSProxy {
    /**
     * @dev Storage slot with the address of the current implementation.
     * This is the keccak-256 hash of "eip1967.proxy.implementation" subtracted by 1
     */
    bytes32 private constant IMPLEMENTATION_SLOT = 0x360894a13ba1a3210667c828492db98dca3e2076cc3735a920a3ca505d382bbc;

    /**
     * @dev Storage slot with the admin of the contract.
     * This is the keccak-256 hash of "eip1967.proxy.admin" subtracted by 1
     */
    bytes32 private constant ADMIN_SLOT = 0xb53127684a568b3173ae13b9f8a6016e243e63b6e8ee1178d6a717850b5d6103;

    /**
     * @dev Emitted when the implementation is upgraded.
     */
    event Upgraded(address indexed implementation);

    /**
     * @dev Emitted when the admin changes.
     */
    event AdminChanged(address previousAdmin, address newAdmin);

    /**
     * @dev Constructor sets the initial implementation and admin
     * @param implementation Address of the initial implementation contract
     * @param admin Address of the proxy admin
     * @param data Initialization data for delegatecall to implementation
     */
    constructor(address implementation, address admin, bytes memory data) {
        // Set implementation
        assembly {
            sstore(IMPLEMENTATION_SLOT, implementation)
        }
        emit Upgraded(implementation);

        // Set admin
        assembly {
            sstore(ADMIN_SLOT, admin)
        }
        emit AdminChanged(address(0), admin);

        // Initialize implementation
        if (data.length > 0) {
            (bool success,) = implementation.delegatecall(data);
            require(success, "Initialization failed");
        }
    }

    /**
     * @dev Fallback function that delegates calls to the implementation
     */
    fallback() external payable {
        assembly {
            // Load implementation address from storage
            let implementation := sload(IMPLEMENTATION_SLOT)

            // Copy calldata to memory
            calldatacopy(0, 0, calldatasize())

            // Delegatecall to implementation
            let result := delegatecall(gas(), implementation, 0, calldatasize(), 0, 0)

            // Copy return data to memory
            returndatacopy(0, 0, returndatasize())

            // Return or revert based on result
            switch result
            case 0 { revert(0, returndatasize()) }
            default { return(0, returndatasize()) }
        }
    }

    /**
     * @dev Receive function
     */
    receive() external payable {}
}
