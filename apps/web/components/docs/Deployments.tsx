import { CopyButton } from "@/components/docs/CopyButton";
import { Icon } from "@/components/ui/Icon";
import { type ContractLink, getDeployments, SOURCE_NOTE } from "@/lib/docs/deployments";

/** An address linked to its explorer page, with a copy button and (for Paralock's own contracts) its verified source. */
function Address({ c, label }: { c: ContractLink; label: string }) {
  return (
    <span className="deploy__value">
      <span className="deploy__addr">
        {c.explorer ? (
          <a href={c.explorer} target="_blank" rel="noreferrer" title="Open in the block explorer">
            <code>{c.address}</code>
          </a>
        ) : (
          <code>{c.address}</code>
        )}
        <CopyButton value={c.address} label={`Copy ${label} address`} className="doc-copy doc-copy--sm" />
      </span>
      {c.source && (
        <a className="deploy__source" href={c.source} target="_blank" rel="noreferrer">
          Source
          <Icon name="external" size={11} />
        </a>
      )}
    </span>
  );
}

/**
 * The public testnet deployments, generated from `contracts/deployments/*.json` (replaces `<!-- deployments -->`).
 * One group per network: both escrows, the quote token and the block to index from.
 */
export function Deployments() {
  const deployments = getDeployments();
  if (deployments.length === 0) return <p>No public deployments found.</p>;
  return (
    <div className="deploy">
      <div className="doc-table deploy__scroll">
        <table>
          <caption className="sr-only">Paralock escrow contracts on public testnets</caption>
          {deployments.map((d) => (
            <tbody key={d.chainId}>
              <tr className="deploy__net">
                <th colSpan={2} scope="colgroup">
                  <span className="deploy__name">{d.network}</span>
                  <span className="deploy__chain">
                    Chain ID <span className="num">{d.chainId}</span>
                  </span>
                  {!d.explorerUrl && <span className="deploy__chain">No official explorer</span>}
                </th>
              </tr>
              <tr>
                <th scope="row">ParalockHTLC</th>
                <td>
                  <Address c={d.htlc} label={`${d.network} ParalockHTLC`} />
                </td>
              </tr>
              <tr>
                <th scope="row">ParalockAdaptorEscrow</th>
                <td>
                  <Address c={d.adaptorEscrow} label={`${d.network} ParalockAdaptorEscrow`} />
                </td>
              </tr>
              <tr>
                <th scope="row">
                  {d.quoteToken.symbol}
                  {d.quoteToken.testToken && <span className="deploy__tag">MockUSD test token</span>}
                </th>
                <td>
                  <Address c={d.quoteToken} label={`${d.network} ${d.quoteToken.symbol}`} />
                </td>
              </tr>
              <tr>
                <th scope="row">Start block</th>
                <td>
                  <span className="num">{d.startBlock}</span>
                </td>
              </tr>
            </tbody>
          ))}
        </table>
      </div>
      <p className="deploy__note">{SOURCE_NOTE}</p>
    </div>
  );
}
