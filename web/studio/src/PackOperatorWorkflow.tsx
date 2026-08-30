import { useState } from "react";

import {
  EMPTY_PACK_OPERATOR_WORKFLOW,
  PackOperatorReceiptError,
  applyPackOperatorReceipt,
  commandForPackOperatorPhase,
  confirmPackDaemonStopped,
  parsePackOperatorReceipt,
  type PackOperatorWorkflowState,
} from "./packOperator";

export function PackOperatorWorkflow() {
  const [workflow, setWorkflow] = useState<PackOperatorWorkflowState>(EMPTY_PACK_OPERATOR_WORKFLOW);
  const [receiptText, setReceiptText] = useState("");
  const [error, setError] = useState<string | null>(null);

  const consume = () => {
    const receipt = parsePackOperatorReceipt(receiptText);
    if (receipt === null) {
      setError("Receipt is not an exact, bounded worldstreamctl Pack receipt.");
      return;
    }
    try {
      const next = applyPackOperatorReceipt(workflow, receipt);
      setWorkflow(next);
      setReceiptText("");
      setError(null);
    } catch (cause) {
      setError(cause instanceof PackOperatorReceiptError
        ? cause.message
        : "The receipt could not advance this exact bundle workflow.");
    }
  };

  return (
    <section className="pack-operator-workflow" id="pack-operator" aria-labelledby="pack-operator-title">
      <div className="pack-operator-heading">
        <div>
          <p className="eyebrow">Operations · Activity Pack Bundles</p>
          <h2 id="pack-operator-title">Approve one exact bundle offline</h2>
          <p>
            Run each command in an owner-controlled terminal, then paste only its typed JSON
            receipt. Studio never receives a path, bundle bytes, shell, approval authority,
            or signing key.
          </p>
        </div>
        <span className={workflow.phase === "ready" ? "bounded-chip is-ready" : "bounded-chip"}>
          {workflow.phase === "ready" ? "Start-ready" : `${workflow.receipts.length}/7 receipts`}
        </span>
      </div>

      <ol className="pack-operator-steps">
        {[
          ["inspect", "Verify exact archive bytes"],
          ["approve", "Record target-local approval"],
          ["install", "Retain original bytes"],
          ["inventory", "Reverify local inventory"],
          ["set_selectable", "Permit new Room selection"],
          ["selected_inventory", "Pin selectable inventory"],
          ["restart_readiness", "Admit Component and Replay Rooms"],
        ].map(([operation, label], index) => (
          <li className={index < workflow.receipts.length ? "is-complete" : index === workflow.receipts.length ? "is-current" : ""} key={operation}>
            <strong>{label}</strong><span>{operation}</span>
          </li>
        ))}
      </ol>

      {workflow.bundle_digest === null ? null : (
        <dl className="pack-operator-identities">
          <div><dt>Pack</dt><dd>{workflow.pack_id} {workflow.explanatory_version}</dd></div>
          <div><dt>Physical bundle</dt><dd><code>{workflow.bundle_digest}</code></dd></div>
          <div><dt>Semantic revision</dt><dd><code>{workflow.revision_digest}</code></dd></div>
        </dl>
      )}

      <div className="pack-operator-command">
        <strong>{workflow.phase === "ready" ? "Safe next action" : "Run next"}</strong>
        <code>{commandForPackOperatorPhase(workflow)}</code>
      </div>

      {workflow.phase === "awaiting_approval" && !workflow.daemon_stop_confirmed ? (
        <div className="pack-operator-stop-gate" role="group" aria-label="Daemon stop confirmation">
          <strong>Required before local mutation</strong>
          <p>
            Stop the serving <code>worldstreamd</code> process with its ordinary service
            manager. Studio cannot stop it, inspect process state, or hold Host authority.
          </p>
          <button
            onClick={() => {
              try {
                setWorkflow(confirmPackDaemonStopped(workflow));
                setError(null);
              } catch (cause) {
                setError(cause instanceof PackOperatorReceiptError
                  ? cause.message
                  : "Stop confirmation failed closed.");
              }
            }}
            type="button"
          >
            I stopped worldstreamd
          </button>
        </div>
      ) : null}

      {workflow.phase === "ready" ? (
        <p className="pack-operator-ready" role="status">
          Exact original bytes, production Component admission, and configured-store executable
          Replay passed. Starting the stopped daemon may now publish the immutable startup registry.
        </p>
      ) : workflow.phase === "awaiting_approval" && !workflow.daemon_stop_confirmed ? null : (
        <div className="pack-operator-receipt-input">
          <label htmlFor="pack-operator-receipt">Typed JSON receipt from worldstreamctl</label>
          <textarea
            id="pack-operator-receipt"
            onChange={(event) => setReceiptText(event.target.value)}
            placeholder='{"schema":"worldstream/pack-operator-receipt/v1",…}'
            spellCheck={false}
            value={receiptText}
          />
          <button disabled={receiptText.trim().length === 0} onClick={consume} type="button">
            Verify and advance
          </button>
        </div>
      )}

      {error === null ? null : <p className="pack-operator-error" role="alert">{error}</p>}
      <p className="pack-boundary-note">
        The daemon still loads installed Packs only at startup. Studio is neither a Participant,
        a Pack authoring surface, nor a second rules engine.
      </p>
    </section>
  );
}
