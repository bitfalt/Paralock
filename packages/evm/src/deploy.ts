import {
  type Account,
  type Address,
  createPublicClient,
  getAddress,
  type Hex,
  http,
  keccak256,
  parseAbi,
  stringToHex,
  type TransactionReceipt,
} from "viem";
import {
  MOCK_USD_ARTIFACT,
  PARALOCK_ADAPTOR_ESCROW_ARTIFACT,
  PARALOCK_HTLC_ARTIFACT,
  type ParalockContractArtifact,
} from "./artifacts.js";
import { isTempoChainId, TEMPO_TOKENS } from "./chain.js";
import { createEvmClients, type EvmConnectionOptions } from "./client.js";
import { TransactionRevertedError } from "./internal.js";
import { knownNetwork } from "./networks.js";

/** Tempo's TIP-20 factory precompile. */
const TIP20_FACTORY: Address = "0x20Fc000000000000000000000000000000000000";
/** Salt of the local OUSD stand-in, as in `contracts/script/Deploy.s.sol` (`keccak256("paralock/ousd")`). */
const LOCAL_OUSD_SALT = keccak256(stringToHex("paralock/ousd"));
/** 1B USD (6 decimals) per minted account, as in `Deploy.s.sol`. */
const LOCAL_MINT = 1_000_000_000_000_000n;

const tip20FactoryAbi = parseAbi([
  "function createToken(string name, string symbol, string currency, address quoteToken, address admin, bytes32 salt) returns (address)",
  "function isTIP20(address token) view returns (bool)",
  "function getTokenAddress(address sender, bytes32 salt) pure returns (address)",
]);
const tip20IssuerAbi = parseAbi([
  "function ISSUER_ROLE() view returns (bytes32)",
  "function grantRole(bytes32 role, address account)",
  "function mint(address to, uint256 amount)",
]);

/** Paralock's contracts on one chain: the shape of `contracts/deployments/<chainId>.json`. */
export interface ParalockDeployment {
  /** ParalockAdaptorEscrow (adaptor mode). */
  adaptorEscrow: Address;
  /** Chain family (`tempo`, `base`, …), when known. */
  chain?: string;
  chainId: number;
  /** The account that deployed the contracts (and the only one that can mint a local MockUSD). */
  deployer: Address;
  /** ParalockHTLC (htlc mode). */
  htlc: Address;
  /** Symbol of the swap stablecoin, a key of `tokens`: `OUSD` on Tempo, `USDC` elsewhere. */
  quoteToken: string;
  /** First block to scan for escrow events. */
  startBlock: number;
  /** Stablecoins by symbol. */
  tokens: Record<string, Address>;
}

/** Options of `deployParalockContracts`. */
export interface DeployParalockContractsOptions extends Omit<EvmConnectionOptions, "chainId" | "account"> {
  /** Expected chain id. Default: asked from the node. */
  chainId?: number;
  /** Deployer: a viem Account or a private key. On the Paralock devnet, anvil dev account 0. */
  account: Account | Hex;
  /** Chain family recorded in the deployment (`tempo`, `base`, …). Default: `tempo` on Tempo chains, else none. */
  chain?: string;
  /**
   * An earlier deployment on this chain (e.g. a parsed `contracts/deployments/<chainId>.json`). Its ParalockHTLC and
   * ParalockAdaptorEscrow are reused when the chain still holds exactly this build's runtime code, and its MockUSD when
   * it still exists and `account` is its minter. Reused contracts keep the earlier `startBlock`.
   */
  previous?: Partial<Pick<ParalockDeployment, "htlc" | "adaptorEscrow" | "startBlock" | "tokens">>;
  /** Accounts that receive `mintAmount` of a newly created stablecoin. Default: the deployer. */
  mintTo?: readonly Address[];
  /** Base units minted to each of `mintTo`. Default 1 000 000 000 USD (`1_000_000_000_000_000n`). */
  mintAmount?: bigint;
  /** Progress messages ("deployed ParalockHTLC at 0x…", "reusing …"). */
  log?: (message: string) => void;
}

/** Result of `deployParalockContracts`. */
export interface DeployParalockContractsResult {
  deployment: ParalockDeployment;
  /** What already existed and was kept instead of being created. */
  reused: { htlc: boolean; adaptorEscrow: boolean; quoteToken: boolean };
}

/**
 * Deploys ParalockHTLC, ParalockAdaptorEscrow and a local stablecoin from the embedded artifacts, the way
 * `contracts/script/Deploy.s.sol` does on the Paralock devnet, without Foundry:
 *
 * - On a Tempo chain (`anvil --network tempo`): an "OUSD" stand-in created through the TIP-20 factory precompile at an
 *   address fixed by (deployer, salt), so it is reused once it exists. The new token is minted to `mintTo`.
 * - Elsewhere: a MockUSD "USDC" (6 decimals, only the deployer can mint), minted to `mintTo`.
 *
 * Contracts in `previous` are reused when still current (see `previous`). Local chains only: it refuses the public
 * networks in `KNOWN_NETWORKS`; deploy there with `contracts/script/Deploy.s.sol`.
 */
export async function deployParalockContracts(options: DeployParalockContractsOptions): Promise<DeployParalockContractsResult> {
  const chainId = options.chainId ?? (await createPublicClient({ transport: http(options.rpcUrl) }).getChainId());
  const known = knownNetwork(chainId);
  if (known) {
    throw new Error(
      `chain ${chainId} is ${known.name}: deployParalockContracts only deploys to local chains (it creates a mintable test stablecoin). Use contracts/script/Deploy.s.sol.`,
    );
  }
  const tempo = options.tempo ?? (options.feeToken !== undefined || isTempoChainId(chainId));
  const { publicClient, walletClient } = createEvmClients({ ...options, chainId, tempo });
  if (!walletClient) throw new Error("deployParalockContracts requires an account");
  const deployer = walletClient.account.address;
  const log = options.log ?? (() => undefined);
  const previous = options.previous ?? {};
  const mintTo = options.mintTo ?? [deployer];
  const mintAmount = options.mintAmount ?? LOCAL_MINT;
  const receiptTimeoutMs = options.receiptTimeoutMs ?? 180_000;

  const confirm = async (hash: Hex): Promise<TransactionReceipt> => {
    const receipt = await publicClient.waitForTransactionReceipt({ hash, timeout: receiptTimeoutMs });
    if (receipt.status !== "success") throw new TransactionRevertedError(hash);
    return receipt;
  };
  const deploy = async (artifact: ParalockContractArtifact): Promise<Address> => {
    const hash = await walletClient.deployContract({ abi: artifact.abi, bytecode: artifact.bytecode });
    const { contractAddress } = await confirm(hash);
    if (!contractAddress) throw new Error(`${artifact.contractName} deployment ${hash} created no contract`);
    const address = getAddress(contractAddress);
    log(`deployed ${artifact.contractName} at ${address}`);
    return address;
  };
  /** True if `address` holds exactly `artifact`'s runtime code. */
  const isCurrent = async (address: Address | undefined, artifact: ParalockContractArtifact): Promise<boolean> => {
    if (!address) return false;
    const code = await publicClient.getCode({ address });
    return code?.toLowerCase() === artifact.deployedBytecode.toLowerCase();
  };
  /** True if `address` is a MockUSD that `deployer` can mint. */
  const isOwnMockUsd = async (address: Address | undefined): Promise<boolean> => {
    if (!address || !(await publicClient.getCode({ address }))) return false;
    try {
      const minter = await publicClient.readContract({ address, abi: MOCK_USD_ARTIFACT.abi, functionName: "minter" });
      return getAddress(minter) === deployer;
    } catch {
      return false;
    }
  };

  const reused = {
    htlc: await isCurrent(previous.htlc, PARALOCK_HTLC_ARTIFACT),
    adaptorEscrow: await isCurrent(previous.adaptorEscrow, PARALOCK_ADAPTOR_ESCROW_ARTIFACT),
    quoteToken: false,
  };
  // Reused contracts keep the earlier start block, so event scans still cover them.
  const startBlock =
    (reused.htlc || reused.adaptorEscrow) && previous.startBlock !== undefined
      ? previous.startBlock
      : Number(await publicClient.getBlockNumber());

  const htlc = reused.htlc ? getAddress(previous.htlc!) : await deploy(PARALOCK_HTLC_ARTIFACT);
  if (reused.htlc) log(`reusing ParalockHTLC at ${htlc}`);
  const adaptorEscrow = reused.adaptorEscrow
    ? getAddress(previous.adaptorEscrow!)
    : await deploy(PARALOCK_ADAPTOR_ESCROW_ARTIFACT);
  if (reused.adaptorEscrow) log(`reusing ParalockAdaptorEscrow at ${adaptorEscrow}`);

  const quoteToken = tempo ? "OUSD" : "USDC";
  let token: Address;
  if (tempo) {
    token = await publicClient.readContract({
      address: TIP20_FACTORY,
      abi: tip20FactoryAbi,
      functionName: "getTokenAddress",
      args: [deployer, LOCAL_OUSD_SALT],
    });
    reused.quoteToken = await publicClient.readContract({
      address: TIP20_FACTORY,
      abi: tip20FactoryAbi,
      functionName: "isTIP20",
      args: [token],
    });
    if (reused.quoteToken) {
      log(`reusing the local OUSD stand-in at ${token}`);
    } else {
      await confirm(
        await walletClient.writeContract({
          address: TIP20_FACTORY,
          abi: tip20FactoryAbi,
          functionName: "createToken",
          args: ["Open USD (Paralock local)", "OUSD", "USD", TEMPO_TOKENS.PathUSD, deployer, LOCAL_OUSD_SALT],
        }),
      );
      const issuer = await publicClient.readContract({ address: token, abi: tip20IssuerAbi, functionName: "ISSUER_ROLE" });
      await confirm(
        await walletClient.writeContract({ address: token, abi: tip20IssuerAbi, functionName: "grantRole", args: [issuer, deployer] }),
      );
      for (const to of mintTo) {
        await confirm(
          await walletClient.writeContract({ address: token, abi: tip20IssuerAbi, functionName: "mint", args: [to, mintAmount] }),
        );
      }
      log(`created the local OUSD stand-in at ${token}`);
    }
  } else {
    reused.quoteToken = await isOwnMockUsd(previous.tokens?.USDC);
    if (reused.quoteToken) {
      token = getAddress(previous.tokens!.USDC!);
      log(`reusing MockUSD at ${token}`);
    } else {
      token = await deploy(MOCK_USD_ARTIFACT);
      for (const to of mintTo) {
        await confirm(
          await walletClient.writeContract({ address: token, abi: MOCK_USD_ARTIFACT.abi, functionName: "mint", args: [to, mintAmount] }),
        );
      }
    }
  }

  const tokens: Record<string, Address> = tempo
    ? {
        AlphaUSD: TEMPO_TOKENS.AlphaUSD,
        BetaUSD: TEMPO_TOKENS.BetaUSD,
        OUSD: getAddress(token),
        PathUSD: TEMPO_TOKENS.PathUSD,
        ThetaUSD: TEMPO_TOKENS.ThetaUSD,
      }
    : { USDC: token };
  const chain = options.chain ?? (tempo ? "tempo" : undefined);
  // Keys in alphabetical order, like the files Deploy.s.sol writes.
  const deployment: ParalockDeployment = {
    adaptorEscrow,
    ...(chain === undefined ? {} : { chain }),
    chainId,
    deployer,
    htlc,
    quoteToken,
    startBlock,
    tokens,
  };
  return { deployment, reused };
}
