/**
 * The TypeScript half of the Rust ↔ TypeScript interop test (`crates/paralock-evm/tests/interop.rs`). Run from the
 * repository root with the built `packages/evm/dist`:
 *
 *   npx tsx crates/paralock-evm/tests/interop/interop.ts <input.json>
 *
 * Phase "act": reads the swaps the Rust client locked (HTLC and adaptor escrow) with `@paralock/evm`, reports what
 * it sees (swap fields, lock block and transaction, commitment, id, claim digest), claims both (the HTLC with the
 * preimage, the escrow with the Rust-made signature), then locks one HTLC swap and one escrow swap of its own for
 * Rust to read back and claim.
 *
 * Phase "verify": waits for Rust's claims of the TypeScript locks and reports the revealed preimage and signature.
 *
 * Prints one JSON object (bigints as decimal strings) on stdout.
 */
import { readFileSync } from "node:fs";
import * as evm from "../../../../packages/evm/dist/index.js";

type Hex = `0x${string}`;

interface SwapJson {
  [key: string]: string;
}

const input = JSON.parse(readFileSync(process.argv[2]!, "utf8"));
const common = {
  rpcUrl: input.rpcUrl as string,
  chainId: input.chainId as number,
  fromBlock: BigInt(input.fromBlock),
  pollingIntervalMs: 100,
};

function htlcSwap(j: SwapJson): evm.HtlcSwap {
  return {
    hashlock: j.hashlock as Hex,
    token: j.token as Hex,
    sender: j.sender as Hex,
    recipient: j.recipient as Hex,
    amount: BigInt(j.amount!),
    expiry: BigInt(j.expiry!),
  };
}

function adaptorSwap(j: SwapJson): evm.AdaptorSwap {
  return {
    token: j.token as Hex,
    sender: j.sender as Hex,
    recipient: j.recipient as Hex,
    signer: j.signer as Hex,
    amount: BigInt(j.amount!),
    expiry: BigInt(j.expiry!),
    salt: j.salt as Hex,
  };
}

function htlcClient(account: Hex) {
  return evm.createEvmHtlc({ ...common, contract: input.htlc, account });
}

function escrowClient(account: Hex) {
  return evm.createAdaptorEscrow({ ...common, contract: input.adaptorEscrow, account });
}

const replacer = (_key: string, value: unknown) => (typeof value === "bigint" ? value.toString() : value);

async function act() {
  const relayerHtlc = htlcClient(input.relayerKey);
  const relayerEscrow = escrowClient(input.relayerKey);
  const makerHtlc = htlcClient(input.makerKey);
  const makerEscrow = escrowClient(input.makerKey);

  // 1. What the Rust client locked, as @paralock/evm sees it.
  const rustHtlc = htlcSwap(input.rustHtlc.swap);
  const htlcLock = await relayerHtlc.getLock(rustHtlc.hashlock);
  if (!htlcLock) throw new Error("the Rust HTLC lock is not visible");
  const rustAdaptor = adaptorSwap(input.rustAdaptor.swap);
  const adaptorId = evm.adaptorSwapId(rustAdaptor);
  const adaptorLock = await relayerEscrow.getLock(input.rustAdaptor.id);
  if (!adaptorLock) throw new Error("the Rust escrow lock is not visible");

  const htlcStateBeforeClaim = await relayerHtlc.getState(rustHtlc.hashlock);

  // 2. Claim both.
  const htlcClaim = await relayerHtlc.claim(htlcLock.swap, input.rustHtlc.preimage);
  const adaptorClaim = await relayerEscrow.claim(adaptorLock.swap, input.rustAdaptor.signature);

  // 3. Lock one of each for Rust.
  const now = (await makerHtlc.publicClient.getBlock()).timestamp;
  const tsHtlc: evm.HtlcSwap = { ...htlcSwap(input.tsHtlc), expiry: now + 3600n };
  await makerHtlc.approveIfNeeded(tsHtlc.token, tsHtlc.amount);
  const tsHtlcLock = await makerHtlc.lock(tsHtlc);
  const tsAdaptor: evm.AdaptorSwap = { ...adaptorSwap(input.tsAdaptor), expiry: now + 3600n };
  await makerEscrow.approveIfNeeded(tsAdaptor.token, tsAdaptor.amount);
  const tsAdaptorLock = await makerEscrow.lock(tsAdaptor);

  return {
    rustHtlc: {
      swap: htlcLock.swap,
      blockNumber: htlcLock.blockNumber,
      transactionHash: htlcLock.transactionHash,
      commitment: evm.commitmentOf(htlcLock.swap),
      state: htlcStateBeforeClaim,
      claimTx: htlcClaim.hash,
    },
    rustAdaptor: {
      swap: adaptorLock.swap,
      blockNumber: adaptorLock.blockNumber,
      transactionHash: adaptorLock.transactionHash,
      id: adaptorId,
      claimDigest: relayerEscrow.claimDigest(adaptorId),
      claimTx: adaptorClaim.hash,
    },
    tsHtlc: { swap: tsHtlc, lockTx: tsHtlcLock.hash, blockNumber: tsHtlcLock.blockNumber },
    tsAdaptor: { swap: tsAdaptor, id: tsAdaptorLock.id, lockTx: tsAdaptorLock.hash, blockNumber: tsAdaptorLock.blockNumber },
  };
}

async function verify() {
  const htlc = htlcClient(input.relayerKey);
  const escrow = escrowClient(input.relayerKey);
  const preimage = await htlc.waitForClaimPreimage(input.tsHtlc.hashlock, { timeoutMs: 10_000 });
  const signature = await escrow.waitForClaimSignature(input.tsAdaptor.id, { timeoutMs: 10_000 });
  return { preimage, signature, htlcState: await htlc.getState(input.tsHtlc.hashlock), escrowState: await escrow.getState(input.tsAdaptor.id) };
}

const result = input.phase === "verify" ? await verify() : await act();
console.log(JSON.stringify(result, replacer));
