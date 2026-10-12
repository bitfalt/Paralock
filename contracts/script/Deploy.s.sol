// SPDX-License-Identifier: MIT
pragma solidity ^0.8.24;

import {Script, console} from "forge-std/Script.sol";
import {VmSafe} from "forge-std/Vm.sol";
import {ITIP20} from "tempo-std/interfaces/ITIP20.sol";
import {ITIP20RolesAuth} from "tempo-std/interfaces/ITIP20RolesAuth.sol";
import {StdPrecompiles} from "tempo-std/StdPrecompiles.sol";
import {StdTokens} from "tempo-std/StdTokens.sol";
import {ParalockHTLC} from "../src/ParalockHTLC.sol";
import {ParalockAdaptorEscrow} from "../src/ParalockAdaptorEscrow.sol";
import {MockUSD} from "../src/MockUSD.sol";

/// @notice Deploys ParalockHTLC and ParalockAdaptorEscrow and writes `deployments/<chainId>.json`.
///
///         Idempotent: contracts recorded in an existing `deployments/<chainId>.json` are reused when the chain still
///         holds exactly their current bytecode, so re-running only deploys what is missing or changed.
///
///         Stablecoin per chain:
///         - Tempo: Open USD (OUSD). On a local Tempo chain (anvil `--network tempo`, chain id 31337) it creates an
///           "OUSD" stand-in through the TIP20Factory precompile and mints it to anvil dev accounts 0 and 1; factory
///           addresses are deterministic in (deployer, salt), so re-runs reuse it.
///         - Local Base / Ethereum (anvil, chain ids 31338 / 31339): a MockUSD "USDC" (6 decimals) minted to anvil
///           dev accounts 0 and 1.
///         - Other chains: the USDC address from the `USDC` environment variable.
///
///         Tempo vs. plain EVM is derived from the chain id; set `PARALOCK_EVM_CHAIN=tempo|base|ethereum` to override.
///
///         Non-Tempo chains need forge's Ethereum EVM (`FOUNDRY_NETWORK=ethereum`), since foundry.toml defaults to Tempo.
///
///         Local Tempo:  forge script script/Deploy.s.sol --rpc-url tempo_local --broadcast
///         Local Base:   FOUNDRY_NETWORK=ethereum forge script script/Deploy.s.sol --rpc-url base_local --broadcast
///         Testnet:      PRIVATE_KEY=0x… forge script script/Deploy.s.sol --rpc-url moderato --broadcast
contract Deploy is Script {
    uint256 internal constant TEMPO_LOCAL_CHAIN_ID = 31337;
    uint256 internal constant BASE_LOCAL_CHAIN_ID = 31338;
    uint256 internal constant ETHEREUM_LOCAL_CHAIN_ID = 31339;
    uint256 internal constant TEMPO_MAINNET_CHAIN_ID = 4217;
    uint256 internal constant TEMPO_MODERATO_CHAIN_ID = 42431;

    /// @dev Anvil's first two dev accounts (the standard `test test … junk` mnemonic).
    uint256 internal constant ANVIL_DEV0_KEY = 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    address internal constant ANVIL_DEV1 = 0x70997970C51812dc3A010C7d01b50e0d17dc79C8;

    /// @dev Open USD on Tempo mainnet and Moderato testnet.
    address internal constant OUSD = 0x20c0000000000000000000006a37DA5C996874BE;
    bytes32 internal constant LOCAL_OUSD_SALT = keccak256("paralock/ousd");
    uint256 internal constant LOCAL_MINT = 1_000_000_000e6; // 1B USD per dev account

    struct Deployment {
        string chain;
        bool tempo;
        bool local;
        address deployer;
        address htlc;
        address adaptorEscrow;
        /// @dev OUSD on Tempo, USDC elsewhere.
        address usd;
        uint256 startBlock;
    }

    function run() external returns (Deployment memory d) {
        d.chain = _chainName();
        d.tempo = keccak256(bytes(d.chain)) == keccak256("tempo");
        d.local = block.chainid == TEMPO_LOCAL_CHAIN_ID || block.chainid == BASE_LOCAL_CHAIN_ID
            || block.chainid == ETHEREUM_LOCAL_CHAIN_ID;
        uint256 key = d.local ? vm.envOr("PRIVATE_KEY", ANVIL_DEV0_KEY) : vm.envUint("PRIVATE_KEY");
        d.deployer = vm.addr(key);
        _loadPrevious(d);

        // Reused contracts keep the previous (earlier) start block, so event scans still cover them.
        if (d.startBlock == 0) d.startBlock = block.number;

        vm.startBroadcast(key);
        if (d.htlc == address(0)) d.htlc = address(new ParalockHTLC());
        if (d.adaptorEscrow == address(0)) d.adaptorEscrow = address(new ParalockAdaptorEscrow());
        if (d.tempo) d.usd = d.local ? _localOusd(d.deployer) : OUSD;
        else if (d.usd == address(0)) d.usd = d.local ? _localUsdc() : vm.envAddress("USDC");
        vm.stopBroadcast();

        // A dry run must not overwrite the record of what is actually deployed.
        if (!vm.isContext(VmSafe.ForgeContext.ScriptDryRun)) _writeDeployment(d);
    }

    /// @dev "tempo", "base" or "ethereum": `PARALOCK_EVM_CHAIN`, else derived from the chain id.
    function _chainName() internal view returns (string memory) {
        string memory forced = vm.envOr("PARALOCK_EVM_CHAIN", string(""));
        if (bytes(forced).length != 0) return forced;
        uint256 id = block.chainid;
        if (id == TEMPO_LOCAL_CHAIN_ID || id == TEMPO_MAINNET_CHAIN_ID || id == TEMPO_MODERATO_CHAIN_ID) {
            return "tempo";
        }
        if (id == BASE_LOCAL_CHAIN_ID || id == 8453 || id == 84532) return "base";
        return "ethereum";
    }

    function _path() internal view returns (string memory) {
        return string.concat(vm.projectRoot(), "/deployments/", vm.toString(block.chainid), ".json");
    }

    /// @dev Reuses contracts from an existing deployment file whose on-chain code is still current.
    function _loadPrevious(Deployment memory d) internal view {
        string memory path = _path();
        if (!vm.isFile(path)) return;
        string memory json = vm.readFile(path);
        address htlc = _jsonAddress(json, ".htlc");
        address escrow = _jsonAddress(json, ".adaptorEscrow");
        if (htlc != address(0) && htlc.codehash == keccak256(type(ParalockHTLC).runtimeCode)) d.htlc = htlc;
        if (escrow != address(0) && escrow.codehash == keccak256(type(ParalockAdaptorEscrow).runtimeCode)) {
            d.adaptorEscrow = escrow;
        }
        if ((d.htlc != address(0) || d.adaptorEscrow != address(0)) && vm.keyExistsJson(json, ".startBlock")) {
            d.startBlock = vm.parseJsonUint(json, ".startBlock");
        }
        if (!d.tempo && d.local) {
            address usdc = _jsonAddress(json, ".tokens.USDC");
            if (usdc.code.length != 0) {
                try MockUSD(usdc).minter() returns (address minter) {
                    if (minter == d.deployer) d.usd = usdc;
                } catch {}
            }
        }
        if (d.htlc != address(0)) console.log("Reusing ParalockHTLC", d.htlc);
        if (d.adaptorEscrow != address(0)) console.log("Reusing ParalockAdaptorEscrow", d.adaptorEscrow);
        if (d.usd != address(0)) console.log("Reusing MockUSD", d.usd);
    }

    function _jsonAddress(string memory json, string memory key) internal view returns (address) {
        return vm.keyExistsJson(json, key) ? vm.parseJsonAddress(json, key) : address(0);
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
        ITIP20(token).mint(deployer, LOCAL_MINT);
        ITIP20(token).mint(ANVIL_DEV1, LOCAL_MINT);
    }

    /// @dev Deploys a MockUSD "USDC" and mints it to the dev accounts (the broadcaster is dev account 0).
    function _localUsdc() internal returns (address) {
        MockUSD usdc = new MockUSD();
        usdc.mint(usdc.minter(), LOCAL_MINT);
        usdc.mint(ANVIL_DEV1, LOCAL_MINT);
        return address(usdc);
    }

    function _writeDeployment(Deployment memory d) internal {
        string memory tokens = "tokens";
        string memory symbol = d.tempo ? "OUSD" : "USDC";
        if (d.tempo) {
            vm.serializeAddress(tokens, "PathUSD", StdTokens.PATH_USD_ADDRESS);
            if (block.chainid != TEMPO_MAINNET_CHAIN_ID) {
                // Pre-funded on local chains, faucet tokens on Moderato.
                vm.serializeAddress(tokens, "AlphaUSD", StdTokens.ALPHA_USD_ADDRESS);
                vm.serializeAddress(tokens, "BetaUSD", StdTokens.BETA_USD_ADDRESS);
                vm.serializeAddress(tokens, "ThetaUSD", StdTokens.THETA_USD_ADDRESS);
            }
        }
        tokens = vm.serializeAddress(tokens, symbol, d.usd);

        string memory root = "deployment";
        vm.serializeString(root, "chain", d.chain);
        vm.serializeUint(root, "chainId", block.chainid);
        vm.serializeAddress(root, "htlc", d.htlc);
        vm.serializeAddress(root, "adaptorEscrow", d.adaptorEscrow);
        vm.serializeAddress(root, "deployer", d.deployer);
        vm.serializeString(root, "quoteToken", symbol);
        vm.serializeUint(root, "startBlock", d.startBlock); // lower bound for log scans
        string memory json = vm.serializeString(root, "tokens", tokens);

        string memory path = _path();
        vm.writeJson(json, path);
        console.log("ParalockHTLC", d.htlc);
        console.log("ParalockAdaptorEscrow", d.adaptorEscrow);
        console.log(symbol, d.usd);
        console.log("Wrote", path);
    }
}
