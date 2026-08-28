import { useState, type FormEvent } from "react";

import {
  createAgentProfileRevisionDraft,
  publishAgentProfile,
  type AgentProfileCatalog,
  type AgentHostContract,
  type AgentProfilePublishOutcome,
  type AgentProfilePublishRequest,
  type AgentProfileRevision,
  type ModelProviderCredentialCatalog,
} from "./agentProfiles";
import type { RunnerTemplateCatalog } from "./runnerTemplates";

export interface AgentProfileBuildProps {
  catalog: AgentProfileCatalog | null;
  onPublished?: (profile: AgentProfileRevision) => void;
  credentials?: ModelProviderCredentialCatalog | null;
  runnerTemplates?: RunnerTemplateCatalog | null;
}

export function AgentProfileBuild({ catalog, onPublished, credentials = null, runnerTemplates = null }: AgentProfileBuildProps) {
  const [draft, setDraft] = useState<AgentProfilePublishRequest>(() =>
    createAgentProfileRevisionDraft());
  const [configuration, setConfiguration] = useState("{}");
  const [outcome, setOutcome] = useState<AgentProfilePublishOutcome | null>(null);
  const [publishing, setPublishing] = useState(false);

  if (catalog === null) {
    return (
      <section aria-labelledby="agent-profile-build-title">
        <p className="eyebrow">Build · Agent execution configuration</p>
        <h2 id="agent-profile-build-title">Agent Profiles</h2>
        <div role="status">
          <strong>Agent Profile catalog unavailable</strong>
          <p>Publishing is disabled until the protected Supervisor store is available.</p>
        </div>
      </section>
    );
  }

  function beginRevision(profile: AgentProfileRevision) {
    const next = createAgentProfileRevisionDraft(profile);
    setDraft(next);
    setConfiguration(JSON.stringify(next.non_secret_configuration, null, 2));
    setOutcome(null);
  }

  function updateManagedHost(
    update: Partial<Exclude<AgentHostContract, { kind: "generic_mcp" }>>,
  ) {
    setDraft((current) => current.host_contract.kind === "generic_mcp" ? current : {
      ...current,
      host_contract: { ...current.host_contract, ...update },
    });
  }

  async function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    let parsed: unknown;
    try {
      parsed = JSON.parse(configuration);
    } catch {
      setOutcome({ kind: "rejected" });
      return;
    }
    if (!isStringRecord(parsed)) {
      setOutcome({ kind: "rejected" });
      return;
    }
    setPublishing(true);
    const result = await publishAgentProfile({ ...draft, non_secret_configuration: parsed });
    setPublishing(false);
    setOutcome(result);
    if (result.kind === "published") onPublished?.(result.profile);
  }

  return (
    <section aria-labelledby="agent-profile-build-title">
      <div>
        <p className="eyebrow">Build · Agent execution configuration</p>
        <h2 id="agent-profile-build-title">Agent Profiles</h2>
        <p>External assignment-bound MCP is the foundational execution path. Managed reference hosts are an explicit post-MVP option.</p>
        <p>Publish non-secret policy and exact kind-bound secret settings as an immutable revision.</p>
      </div>

      {catalog.profiles.length === 0 ? (
        <p>No Agent Profile revisions have been published.</p>
      ) : (
        <ul aria-label="Published Agent Profile revisions">
          {catalog.profiles.map((profile) => (
            <li key={`${profile.profile_id}\0${profile.revision}`}>
              <strong>{profile.display_name}</strong>
              <span>{profile.profile_id} · Revision {profile.revision}</span>
              <span>{profile.host_contract.kind === "managed_reference"
                ? "Managed reference · post-MVP"
                : "External or generic MCP · foundational"}</span>
              <span>{profile.secret_settings.every((setting) => setting.availability === "configured")
                ? "Provider settings configured"
                : "Provider settings need attention"}</span>
              <button type="button" onClick={() => beginRevision(profile)}>Start new revision</button>
            </li>
          ))}
        </ul>
      )}

      <form onSubmit={(event) => void submit(event)}>
        <label>
          Profile ID
          <input
            value={draft.profile_id}
            onChange={(event) => setDraft({ ...draft, profile_id: event.currentTarget.value })}
          />
        </label>
        <fieldset>
          <legend>Execution kind</legend>
          <label>
            <select
              aria-label="Execution kind"
              value={draft.host_contract.kind}
              onChange={(event) => setDraft(setAgentProfileExecutionKind(draft, event.currentTarget.value === "managed_reference" ? "managed_reference" : "generic_mcp"))}
            >
              <option value="generic_mcp">External assignment-bound MCP</option>
              <option value="managed_reference">Managed reference host</option>
            </select>
          </label>
          {draft.host_contract.kind === "managed_reference" ? (
            <>
              <label>Host contract revision<input value={draft.host_contract.host_contract_revision} onChange={(event) => updateManagedHost({ host_contract_revision: event.currentTarget.value })} /></label>
              <label>Approved Runner Template<select value={`${draft.host_contract.runner_template.template_id}\0${draft.host_contract.runner_template.revision}`} onChange={(event) => { const [template_id, revision] = event.currentTarget.value.split("\0"); updateManagedHost({ runner_template: { template_id, revision } }); }}><option value="">Select exact revision</option>{runnerTemplates?.templates.map((template) => <option key={`${template.template_id}\0${template.revision}`} value={`${template.template_id}\0${template.revision}`}>{template.display_name} · {template.revision}</option>)}</select></label>
              <label>Provider loopback address<input value={draft.host_contract.provider_address} onChange={(event) => updateManagedHost({ provider_address: event.currentTarget.value })} /></label>
              <label>Model ID<input value={draft.host_contract.model_id} onChange={(event) => updateManagedHost({ model_id: event.currentTarget.value })} /></label>
              <label>Owner-installed provider credential<select value={draft.managed_provider_credential_id ?? ""} onChange={(event) => setDraft({ ...draft, managed_provider_credential_id: event.currentTarget.value || undefined })}><option value="">Choose owner-installed credential</option>{credentials?.credentials.map((credential) => <option key={credential.credential_id} value={credential.credential_id} disabled={credential.availability !== "configured"}>{credential.display_name} · {credential.availability}</option>)}</select></label>
              <p>The Supervisor resolves the named credential. Studio never receives a credential reference or value.</p>
            </>
          ) : <p>External profiles use the assignment-bound MCP contract and cannot carry managed-provider fields.</p>}
        </fieldset>
        <label>
          Revision
          <input
            value={draft.revision}
            onChange={(event) => setDraft({ ...draft, revision: event.currentTarget.value })}
          />
        </label>
        <label>
          Display name
          <input
            value={draft.display_name}
            onChange={(event) => setDraft({ ...draft, display_name: event.currentTarget.value })}
          />
        </label>
        <label>
          Non-secret configuration (JSON object)
          <textarea value={configuration} onChange={(event) => setConfiguration(event.currentTarget.value)} />
        </label>
        <button type="submit" disabled={publishing}>
          {publishing ? "Publishing…" : "Publish immutable revision"}
        </button>
      </form>

      <PublishOutcome outcome={outcome} />
    </section>
  );
}

export function setAgentProfileExecutionKind(
  draft: AgentProfilePublishRequest,
  kind: AgentHostContract["kind"],
): AgentProfilePublishRequest {
  if (kind === "managed_reference") return {
    ...draft,
    host_contract: { kind: "managed_reference", host_contract_revision: "v1", runner_template: { template_id: "", revision: "" }, provider: "open_ai_compatible", provider_address: "127.0.0.1:11434", model_id: "" },
  };
  const { managed_provider_credential_id: _discarded, ...generic } = draft;
  return { ...generic, host_contract: { kind: "generic_mcp" } };
}

function PublishOutcome({ outcome }: { outcome: AgentProfilePublishOutcome | null }) {
  if (outcome === null) return null;
  if (outcome.kind === "published") {
    return <p role="status">Published exact revision {outcome.profile.revision}.</p>;
  }
  if (outcome.kind === "revision_conflict") {
    return <p role="alert">That revision is immutable. Choose a new revision identity.</p>;
  }
  if (outcome.kind === "rejected") {
    return <p role="alert">Profile settings were rejected. Remove credential material from non-secret configuration.</p>;
  }
  return <p role="alert">Profile publication is unavailable. Verify Supervisor storage and retry.</p>;
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    && Object.values(value).every((setting) => typeof setting === "string");
}
