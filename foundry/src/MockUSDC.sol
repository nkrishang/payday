// SPDX-License-Identifier: MIT
pragma solidity ^0.8.13;

import {ERC20} from "foundry/lib/solady/src/tokens/ERC20.sol";

/// @notice Minimal mintable six-decimal USDC fixture for local Anvil use only.
///
/// It mirrors the two Circle FiatToken controls the sweep worker classifies
/// failures with: a global pause and per-account blacklisting, both readable
/// through the same view functions the real token exposes.
contract MockUSDC is ERC20 {
    bool public paused;
    mapping(address => bool) internal blacklisted;

    function name() public pure override returns (string memory) {
        return "Mock USD Coin";
    }

    function symbol() public pure override returns (string memory) {
        return "USDC";
    }

    function decimals() public pure override returns (uint8) {
        return 6;
    }

    function mint(address to, uint256 amount) external {
        _mint(to, amount);
    }

    function setPaused(bool paused_) external {
        paused = paused_;
    }

    function setBlacklisted(address account, bool blacklisted_) external {
        blacklisted[account] = blacklisted_;
    }

    function isBlacklisted(address account) external view returns (bool) {
        return blacklisted[account];
    }

    function _beforeTokenTransfer(address from, address to, uint256) internal view override {
        require(!paused, "Pausable: paused");
        require(!blacklisted[from] && !blacklisted[to], "Blacklistable: account is blacklisted");
    }
}
