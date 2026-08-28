import { useMemo, useState } from "react";

import { capabilities, filterCapabilities } from "./capabilities";

function unique(field: "area" | "interface" | "status"): string[] {
  return ["All", ...Array.from(new Set(capabilities.map((item) => item[field])))];
}

export function CapabilityExplorer() {
  const [query, setQuery] = useState("");
  const [area, setArea] = useState("All");
  const [interfaceName, setInterfaceName] = useState("All");
  const [status, setStatus] = useState("All");
  const matches = useMemo(
    () => filterCapabilities(capabilities, { query, area, interface: interfaceName, status }),
    [area, interfaceName, query, status],
  );

  return (
    <article className="capability-page">
      <p className="eyebrow">Reference · live inventory</p>
      <h1>Capability explorer</h1>
      <p className="lede">Find the right WorldStream surface and distinguish shipped code from reference integrations, design documents, and deliberately deferred work.</p>
      <div className="capability-filters">
        <label className="capability-query"><span>Search</span><input value={query} onChange={(event) => setQuery(event.target.value)} placeholder="replay, Studio, MCP…" /></label>
        <Filter label="Area" value={area} values={unique("area")} onChange={setArea} />
        <Filter label="Interface" value={interfaceName} values={unique("interface")} onChange={setInterfaceName} />
        <Filter label="Status" value={status} values={unique("status")} onChange={setStatus} />
      </div>
      <div className="result-count"><strong>{matches.length}</strong> of {capabilities.length} capabilities</div>
      <div className="capability-grid">
        {matches.map((item) => (
          <a className="capability-card" href={`#${item.route}`} key={item.name}>
            <div><span>{item.area}</span><span className={`status status-${item.status.toLocaleLowerCase().replaceAll(" ", "-")}`}>{item.status}</span></div>
            <h2>{item.name}</h2>
            <p>{item.description}</p>
            <footer>{item.interface}<b>Open guide →</b></footer>
          </a>
        ))}
      </div>
      {matches.length === 0 && <div className="empty-state"><strong>No matching capability.</strong><button type="button" onClick={() => { setQuery(""); setArea("All"); setInterfaceName("All"); setStatus("All"); }}>Clear filters</button></div>}
    </article>
  );
}

function Filter({ label, value, values, onChange }: { label: string; value: string; values: string[]; onChange: (value: string) => void }) {
  return <label><span>{label}</span><select value={value} onChange={(event) => onChange(event.target.value)}>{values.map((item) => <option value={item} key={item}>{item}</option>)}</select></label>;
}
