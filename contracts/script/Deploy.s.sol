// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {VmSafe} from "forge-std/Vm.sol";
import {ITIP20} from "tempo-std/interfaces/ITIP20.sol";
import {ITIP20RolesAuth} from "tempo-std/interfaces/ITIP20RolesAuth.sol";
import {StdPrecompiles} from "tempo-std/StdPrecompiles.sol";
import {StdTokens} from "tempo-std/StdTokens.sol";
import {ParalockHTLC} from "../src/ParalockHTLC.sol";

/// @notice Deploys ParalockHTLC and writes `deployments/<chainId>.json`.
///
///         On a local Tempo chain (anvil `--network tempo`, chain id 31337) it also creates an "OUSD" stand-in through
///         the TIP20Factory precompile (Open USD only exists on Tempo mainnet/testnet) and mints it to the anvil dev
///         accounts. Re-running reuses the stand-in, because factory addresses are deterministic in (deployer, salt).
///
///         Local:   forge script script/Deploy.s.sol --rpc-url tempo_local --broadcast
///         Testnet: PRIVATE_KEY=0x… forge script script/Deploy.s.sol --rpc-url moderato --broadcast
contract Deploy is Script {
    uint256 internal constant LOCAL_CHAIN_ID = 31337;
    uint256 internal constant MAINNET_CHAIN_ID = 4217;

    /// @dev Anvil's first two dev accounts (the standard `test test … junk` mnemonic).
    uint256 internal constant ANVIL_DEV0_KEY = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    address internal constant ANVIL_DEV1 = 0x70997970C51812dc3A010C7d01b50e0d17dc79C8;

    /// @dev Open USD on Tempo mainnet and Moderato testnet.
    address internal constant OUSD = 0x20c0000000000000000000006a37DA5C996874BE;
    bytes32 internal constant LOCAL_OUSD_SALT = keccak256("paralock/ousd");
    uint256 internal constant LOCAL_OUSD_MINT = 1_000_000_000e6; // 1B OUSD per dev account

    function run() external returns (ParalockHTLC htlc, address ousd) {
        bool local = block.chainid == LOCAL_CHAIN_ID;
        uint256 key = local ? vm.envOr("PRIVATE_KEY", ANVIL_DEV0_KEY) : vm.envUint("PRIVATE_KEY");
        address deployer = vm.addr(key);

        vm.startBroadcast(key);
        htlc = new ParalockHTLC();
        ousd = local ? _localOusd(deployer) : OUSD;
        vm.stopBroadcast();

        // A dry run must not overwrite the record of what is actually deployed.
        if (!vm.isContext(VmSafe.ForgeContext.ScriptDryRun)) _writeDeployment(address(htlc), ousd, deployer);
    }

    /// @dev Creates (once) a USD-denominated TIP-20 quoted in PathUSD and mints it to the dev accounts.
    function _localOusd(address deployer) internal returns (address token) {
        token = StdPrecompiles.TIP20_FACTORY.getTokenAddress(deployer, LOCAL_OUSD_SALT);
        if (StdPrecompiles.TIP20_FACTORY.isTIP20(token)) {
            console.log("Reusing local OUSD stand-in", token);
            return token;
        }
        StdPrecompiles.TIP20_FACTORY
            .createToken("Open USD (Paralock local)", "OUSD", "USD", StdTokens.PATH_USD, deployer, LOCAL_OUSD_SALT);
        ITIP20RolesAuth(token).grantRole(ITIP20(token).ISSUER_ROLE(), deployer);
        ITIP20(token).mint(deployer, LOCAL_OUSD_MINT);
        ITIP20(token).mint(ANVIL_DEV1, LOCAL_OUSD_MINT);
    }

    function _writeDeployment(address htlc, address ousd, address deployer) internal {
        string memory tokens = "tokens";
        vm.serializeAddress(tokens, "PathUSD", StdTokens.PATH_USD_ADDRESS);
        if (block.chainid != MAINNET_CHAIN_ID) {
            // Pre-funded on local chains, faucet tokens on Moderato.
            vm.serializeAddress(tokens, "AlphaUSD", StdTokens.ALPHA_USD_ADDRESS);
            vm.serializeAddress(tokens, "BetaUSD", StdTokens.BETA_USD_ADDRESS);
            vm.serializeAddress(tokens, "ThetaUSD", StdTokens.THETA_USD_ADDRESS);
        }
        tokens = vm.serializeAddress(tokens, "OUSD", ousd);

        string memory root = "deployment";
        vm.serializeUint(root, "chainId", block.chainid);
        vm.serializeAddress(root, "htlc", htlc);
        vm.serializeAddress(root, "deployer", deployer);
        vm.serializeUint(root, "startBlock", block.number); // lower bound for log scans
        string memory json = vm.serializeString(root, "tokens", tokens);

        string memory path = string.concat(vm.projectRoot(), "/deployments/", vm.toString(block.chainid), ".json");
        vm.writeJson(json, path);
        console.log("ParalockHTLC", htlc);
        console.log("OUSD", ousd);
        console.log("Wrote", path);
    }
}
