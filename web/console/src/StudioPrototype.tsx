// PROTOTYPE — throwaway UI used to answer:
// "Which operator-portal information hierarchy should WorldStream Studio use?"
// Four variants, switchable via ?variant=, on the dev-only /prototype/studio route.

import { useEffect, useState } from "react";

import "./studio-prototype.css";

type VariantKey = "A" | "B" | "C" | "D";
type StudioArea = "Home" | "Tasks" | "Build" | "Operations" | "Settings";
type TaskWorkspaceMode = "list" | "detail" | "wizard";
type TaskDetailTab = "Overview" | "Setup" | "Agent & MCP" | "History";
type BuildArea = "Activity Packs" | "Agent Profiles" | "Runner Templates" | "Task Templates";
type OperationsArea = "Topology" | "Processes" | "Storage" | "Attention";

const variants: Array<{ key: VariantKey; name: string }> = [
  { key: "A", name: "Mission Control" },
  { key: "B", name: "Guided Workspace" },
  { key: "C", name: "Live Topology" },
  { key: "D", name: "Recommended Hybrid" },
];

const tasks = [
  {
    id: "canal-shift",
    name: "Canal Shift",
    pack: "Agent Heist 1.1",
    phase: "Lobby",
    state: "Ready to launch",
    tone: "ready",
    seats: "3 / 3 ready",
    runner: "Local MCP · healthy",
    updated: "12 sec ago",
  },
  {
    id: "observatory-run",
    name: "Observatory Run",
    pack: "Agent Heist 1.1",
    phase: "Negotiation",
    state: "Running",
    tone: "live",
    seats: "2 / 3 present",
    runner: "Atlas runner · 2 / 4",
    updated: "Live",
  },
  {
    id: "courier-trial",
    name: "Courier Trial",
    pack: "Agent Heist 1.1",
    phase: "Setup",
    state: "Needs attention",
    tone: "attention",
    seats: "1 seat blocked",
    runner: "Runner unavailable",
    updated: "4 min ago",
  },
  {
    id: "vault-echo",
    name: "Vault Echo",
    pack: "Agent Heist 1.0",
    phase: "Complete",
    state: "Complete",
    tone: "muted",
    seats: "3 / 3 completed",
    runner: "Replay verified",
    updated: "Yesterday",
  },
] as const;

const attentionItems = [
  {
    level: "High",
    title: "Courier Trial lost runner capacity",
    detail: "Insider cannot become ready until a compatible runner returns.",
    action: "Review assignment",
    task: "courier-trial",
  },
  {
    level: "Medium",
    title: "Durable backup is 3 days old",
    detail: "The SQLite store is healthy; the last verified backup is aging.",
    action: "Create backup",
    task: "canal-shift",
  },
  {
    level: "Low",
    title: "One Agent Profile has a missing secret",
    detail: "The profile is not assigned to an active Task.",
    action: "Open profile",
    task: "observatory-run",
  },
] as const;

function variantFromLocation(): VariantKey {
  const candidate = new URLSearchParams(window.location.search).get("variant")?.toUpperCase();
  return candidate === "B" || candidate === "C" || candidate === "D" ? candidate : "A";
}

export function StudioPrototype() {
  const [variant, setVariant] = useState<VariantKey>(variantFromLocation);
  const [selectedTask, setSelectedTask] = useState<string>("canal-shift");
  const [selectedTarget, setSelectedTarget] = useState<string>("worldstreamd");
  const [hybridView, setHybridView] = useState<StudioArea>("Home");
  const [lastAction, setLastAction] = useState<string>("No preview action yet");

  const changeVariant = (next: VariantKey) => {
    const params = new URLSearchParams(window.location.search);
    params.set("variant", next);
    window.history.replaceState(null, "", `${window.location.pathname}?${params.toString()}`);
    setVariant(next);
  };

  const cycleVariant = (direction: number) => {
    const current = variants.findIndex((item) => item.key === variant);
    const next = variants[(current + direction + variants.length) % variants.length];
    changeVariant(next.key);
  };

  useEffect(() => {
    document.title = `WorldStream Studio prototype — ${variant}`;
    const onKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      const tagName = target?.tagName;
      if (tagName === "INPUT" || tagName === "TEXTAREA" || target?.isContentEditable) return;
      if (event.key === "ArrowLeft") cycleVariant(-1);
      if (event.key === "ArrowRight") cycleVariant(1);
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [variant]);

  const previewAction = (action: string) => setLastAction(`Preview only · ${action}`);
  const selectedTaskData = tasks.find((task) => task.id === selectedTask) ?? tasks[0];

  return (
    <main className={`studio-prototype studio-variant-${variant.toLowerCase()}`}>
      <div className="prototype-disclosure">
        <span>Throwaway prototype</span>
        <strong>Static WorldStream fixture</strong>
        <span>No live actions</span>
      </div>

      {variant === "A" ? (
        <VariantA
          onPreviewAction={previewAction}
          onSelectTask={setSelectedTask}
          selectedTask={selectedTask}
        />
      ) : null}
      {variant === "B" ? (
        <VariantB
          onPreviewAction={previewAction}
          onSelectTask={setSelectedTask}
          selectedTask={selectedTask}
        />
      ) : null}
      {variant === "C" ? (
        <VariantC
          onPreviewAction={previewAction}
          onSelectTarget={setSelectedTarget}
          selectedTarget={selectedTarget}
        />
      ) : null}
      {variant === "D" ? (
        <VariantD
          activeView={hybridView}
          onChangeView={setHybridView}
          onPreviewAction={previewAction}
          onSelectTarget={setSelectedTarget}
          onSelectTask={setSelectedTask}
          selectedTarget={selectedTarget}
          selectedTask={selectedTask}
        />
      ) : null}

      {lastAction !== "No preview action yet" ? (
        <div className="prototype-toast" role="status">
          <span className="prototype-toast-dot" aria-hidden="true" />
          {lastAction}
        </div>
      ) : null}

      {import.meta.env.DEV ? (
        <PrototypeSwitcher
          current={variant}
          onChange={changeVariant}
          onCycle={cycleVariant}
          hybridView={hybridView}
          selectedTask={selectedTaskData.name}
          selectedTarget={selectedTarget}
        />
      ) : null}
    </main>
  );
}

interface TaskVariantProps {
  selectedTask: string;
  onSelectTask: (task: string) => void;
  onPreviewAction: (action: string) => void;
}

function VariantA({ selectedTask, onSelectTask, onPreviewAction }: TaskVariantProps) {
  return (
    <div className="a-shell">
      <aside className="a-sidebar">
        <StudioBrand compact />
        <div className="a-workspace">
          <span>Workspace</span>
          <strong>Local workstation</strong>
          <small>Supervisor · connected</small>
        </div>
        <nav aria-label="Studio navigation" className="a-nav">
          <NavButton active glyph="⌂" label="Home" />
          <NavButton glyph="◫" label="Tasks" count="4" />
          <NavButton glyph="◇" label="Build" />
          <NavButton glyph="⌁" label="Operations" count="1" attention />
          <NavButton glyph="⚙" label="Settings" />
        </nav>
        <div className="a-sidebar-footer">
          <div className="health-orb health-good" />
          <div>
            <strong>worldstreamd</strong>
            <span>Online · v0.1.0</span>
          </div>
          <button aria-label="Open daemon controls" onClick={() => onPreviewAction("Open daemon controls")}>•••</button>
        </div>
      </aside>

      <section className="a-content">
        <header className="a-topbar">
          <div>
            <span>Studio</span>
            <strong>/ Home</strong>
          </div>
          <div className="a-top-actions">
            <button className="icon-button" aria-label="Search">⌕</button>
            <button className="icon-button notification-button" aria-label="Notifications">♢<i>3</i></button>
            <button className="primary-button" onClick={() => onPreviewAction("Start a new Task draft")}>＋ New Task</button>
          </div>
        </header>

        <div className="a-page-heading">
          <div>
            <p className="a-eyebrow">Sunday, August 23</p>
            <h1>Everything in one place.</h1>
            <p>Run Tasks, watch capacity, and handle the few things that need you.</p>
          </div>
          <div className="daemon-inline">
            <div className="health-orb health-good" />
            <div><span>Daemon healthy</span><strong>3h 12m uptime</strong></div>
            <button onClick={() => onPreviewAction("Restart worldstreamd")}>Restart</button>
          </div>
        </div>

        <div className="a-metrics" aria-label="Workspace summary">
          <Metric label="Active Tasks" value="3" detail="1 ready to launch" tone="mint" />
          <Metric label="Participants" value="7" detail="5 human · 2 agents" tone="blue" />
          <Metric label="Runner capacity" value="5 / 8" detail="Across 3 runners" tone="violet" />
          <Metric label="Needs attention" value="3" detail="1 blocks a Task" tone="amber" />
        </div>

        <div className="a-mid-grid">
          <section className="a-panel attention-panel">
            <div className="panel-heading-row">
              <div><span className="section-kicker">Attention</span><h2>Needs your review</h2></div>
              <button onClick={() => onPreviewAction("View all attention items")}>View all</button>
            </div>
            <div className="attention-stack">
              {attentionItems.map((item) => (
                <button
                  className="attention-row"
                  key={item.title}
                  onClick={() => {
                    onSelectTask(item.task);
                    onPreviewAction(item.action);
                  }}
                >
                  <span className={`attention-level level-${item.level.toLowerCase()}`}>{item.level}</span>
                  <span><strong>{item.title}</strong><small>{item.detail}</small></span>
                  <b>→</b>
                </button>
              ))}
            </div>
          </section>

          <section className="a-panel capacity-panel">
            <div className="panel-heading-row">
              <div><span className="section-kicker">Runner fleet</span><h2>Capacity</h2></div>
              <span className="soft-badge">Live</span>
            </div>
            <div className="capacity-main">
              <div className="capacity-ring"><strong>62%</strong><span>in use</span></div>
              <div className="capacity-legend">
                <span><i className="legend-used" />5 active</span>
                <span><i className="legend-free" />3 available</span>
                <span><i className="legend-down" />1 runner offline</span>
              </div>
            </div>
            <button className="secondary-button full-button" onClick={() => onPreviewAction("Open Runner operations")}>Open Runner operations</button>
          </section>
        </div>

        <section className="a-panel task-table-panel">
          <div className="panel-heading-row">
            <div><span className="section-kicker">Tasks</span><h2>Current activity</h2></div>
            <button onClick={() => onPreviewAction("Open all Tasks")}>All Tasks</button>
          </div>
          <div className="task-table" role="table" aria-label="Current Tasks">
            <div className="task-table-header" role="row">
              <span>Task</span><span>Phase</span><span>Readiness</span><span>Runner</span><span>Updated</span><span />
            </div>
            {tasks.map((task) => (
              <button
                className={`task-table-row ${selectedTask === task.id ? "is-selected" : ""}`}
                key={task.id}
                onClick={() => onSelectTask(task.id)}
                role="row"
              >
                <span><TaskMark name={task.name} /><span><strong>{task.name}</strong><small>{task.pack}</small></span></span>
                <span>{task.phase}</span>
                <span><StatusPill tone={task.tone}>{task.state}</StatusPill></span>
                <span>{task.runner}</span>
                <span>{task.updated}</span>
                <span>›</span>
              </button>
            ))}
          </div>
        </section>
      </section>
    </div>
  );
}

function VariantB({ selectedTask, onSelectTask, onPreviewAction }: TaskVariantProps) {
  const selected = tasks.find((task) => task.id === selectedTask) ?? tasks[0];
  const columns = [
    { label: "Setup", count: 1, ids: ["courier-trial"] },
    { label: "Ready", count: 1, ids: ["canal-shift"] },
    { label: "Running", count: 1, ids: ["observatory-run"] },
    { label: "Completed", count: 1, ids: ["vault-echo"] },
  ];

  return (
    <div className="b-shell">
      <header className="b-header">
        <StudioBrand />
        <nav aria-label="Studio navigation" className="b-nav">
          <button className="is-active">Home</button><button>Tasks</button><button>Build</button><button>Operations</button><button>Settings</button>
        </nav>
        <div className="b-header-actions">
          <span className="b-system-pill"><i /> Systems healthy</span>
          <button className="b-avatar" aria-label="Local operator">VS</button>
        </div>
      </header>

      <div className="b-content">
        <section className="b-welcome">
          <div>
            <span className="b-overline">Your workspace</span>
            <h1>Good afternoon. What would you like to run?</h1>
            <p>Studio keeps the system details nearby, but leads with the work you are here to do.</p>
          </div>
          <button className="b-new-task" onClick={() => onPreviewAction("Open the guided five-step Task wizard")}>
            <span>＋</span><strong>Start a Task</strong><small>Guided setup · about 2 min</small>
          </button>
        </section>

        <section className="b-next-action">
          <div className="b-next-icon">!</div>
          <div className="b-next-copy">
            <span>Recommended next step</span>
            <h2>Restore a runner for Courier Trial</h2>
            <p>The Insider seat is configured, but its compatible runner went offline four minutes ago.</p>
          </div>
          <div className="b-next-meta">
            <span>Blocks launch</span>
            <strong>1 required seat</strong>
          </div>
          <button onClick={() => {
            onSelectTask("courier-trial");
            onPreviewAction("Review Courier Trial runner assignment");
          }}>Review Task <b>→</b></button>
        </section>

        <div className="b-section-title">
          <div><span>Tasks</span><h2>Follow the flow</h2></div>
          <div className="b-view-controls"><button className="is-active">Board</button><button>List</button><button aria-label="More options">•••</button></div>
        </div>

        <section className="b-board" aria-label="Tasks by lifecycle stage">
          {columns.map((column) => (
            <div className="b-column" key={column.label}>
              <div className="b-column-heading"><span><i className={`column-dot column-${column.label.toLowerCase()}`} />{column.label}</span><b>{column.count}</b></div>
              {column.ids.map((id) => {
                const task = tasks.find((candidate) => candidate.id === id)!;
                return (
                  <button className={`b-task-card ${selectedTask === task.id ? "is-selected" : ""}`} key={task.id} onClick={() => onSelectTask(task.id)}>
                    <div className="b-card-top"><TaskMark name={task.name} /><StatusPill tone={task.tone}>{task.state}</StatusPill></div>
                    <h3>{task.name}</h3>
                    <p>{task.pack}</p>
                    <div className="b-card-rule" />
                    <dl>
                      <div><dt>Phase</dt><dd>{task.phase}</dd></div>
                      <div><dt>Seats</dt><dd>{task.seats}</dd></div>
                    </dl>
                    <footer><span>{task.updated}</span><strong>Open →</strong></footer>
                  </button>
                );
              })}
              <button className="b-column-add" onClick={() => onPreviewAction(`Add a Task in ${column.label}`)}>＋ Add here</button>
            </div>
          ))}
        </section>

        <section className="b-selection-summary" aria-live="polite">
          <div><span>Selected Task</span><strong>{selected.name}</strong><small>{selected.phase} · {selected.seats}</small></div>
          <div className="b-readiness-track"><span style={{ width: selected.tone === "ready" ? "100%" : selected.tone === "live" ? "72%" : selected.tone === "muted" ? "100%" : "36%" }} /></div>
          <button onClick={() => onPreviewAction(`Open ${selected.name}`)}>Open Task</button>
        </section>
      </div>
    </div>
  );
}

interface TopologyVariantProps {
  selectedTarget: string;
  onSelectTarget: (target: string) => void;
  onPreviewAction: (action: string) => void;
}

function VariantC({ selectedTarget, onSelectTarget, onPreviewAction }: TopologyVariantProps) {
  const targetCopy: Record<string, { label: string; type: string; status: string; detail: string }> = {
    supervisor: { label: "Studio Supervisor", type: "Control plane", status: "Healthy", detail: "Serving Studio and reconciling four Tasks." },
    worldstreamd: { label: "worldstreamd", type: "Authoritative runtime", status: "Healthy", detail: "v0.1.0 · SQLite durable · 3h 12m uptime" },
    rooms: { label: "4 Tasks", type: "WorldStream Rooms", status: "1 needs attention", detail: "1 setup · 1 lobby · 1 running · 1 complete" },
    runners: { label: "Runner fleet", type: "3 registered runners", status: "Degraded", detail: "5 / 8 capacity · one runner offline" },
    mcp: { label: "External MCP", type: "Assignment-bound helper", status: "Connected", detail: "Two active agent assignments · ACKs current" },
  };
  const selected = targetCopy[selectedTarget] ?? targetCopy.worldstreamd;

  return (
    <div className="c-shell">
      <header className="c-header">
        <StudioBrand compact />
        <div className="c-environment"><span>Environment</span><button>Local workstation⌄</button></div>
        <div className="c-global-state"><span><i className="pulse-dot" /> Live topology</span><strong>Updated just now</strong></div>
        <div className="c-header-actions"><button aria-label="Search">⌕</button><button aria-label="Notifications">♢<i>3</i></button><button onClick={() => onPreviewAction("Start a new Task draft")}>＋ New Task</button></div>
      </header>

      <aside className="c-rail" aria-label="Studio navigation">
        <button className="is-active"><span>⌂</span><small>Home</small></button>
        <button><span>◫</span><small>Tasks</small></button>
        <button><span>◇</span><small>Build</small></button>
        <button><span>⌁</span><small>Ops</small><i /></button>
        <button><span>⚙</span><small>Settings</small></button>
      </aside>

      <section className="c-workspace">
        <div className="c-canvas-heading">
          <div><span className="c-overline">System map</span><h1>Your WorldStream, live.</h1><p>Select anything to inspect its status and available controls.</p></div>
          <div className="c-map-tools"><button>−</button><span>100%</span><button>＋</button><button>Center</button></div>
        </div>

        <div className="c-map" aria-label="WorldStream system topology">
          <svg className="c-links" viewBox="0 0 920 510" preserveAspectRatio="none" aria-hidden="true">
            <path d="M 180 255 C 250 255, 260 255, 330 255" />
            <path d="M 470 255 C 545 255, 535 142, 620 142" />
            <path d="M 470 255 C 545 255, 535 360, 620 360" />
            <path d="M 760 360 C 815 360, 815 260, 825 260" />
          </svg>
          <TopologyNode className="node-supervisor" label="Studio Supervisor" meta="Control plane" status="Healthy" tone="good" selected={selectedTarget === "supervisor"} onClick={() => onSelectTarget("supervisor")} />
          <TopologyNode className="node-daemon" label="worldstreamd" meta="Authoritative runtime" status="Healthy" tone="good" selected={selectedTarget === "worldstreamd"} onClick={() => onSelectTarget("worldstreamd")} featured />
          <TopologyNode className="node-rooms" label="4 Tasks" meta="Rooms · 3 active" status="1 attention" tone="attention" selected={selectedTarget === "rooms"} onClick={() => onSelectTarget("rooms")} />
          <TopologyNode className="node-runners" label="Runner fleet" meta="5 / 8 capacity" status="Degraded" tone="attention" selected={selectedTarget === "runners"} onClick={() => onSelectTarget("runners")} />
          <TopologyNode className="node-mcp" label="External MCP" meta="2 assignments" status="Connected" tone="good" selected={selectedTarget === "mcp"} onClick={() => onSelectTarget("mcp")} />
          <div className="c-room-chips">
            <button onClick={() => onSelectTarget("rooms")}><i className="chip-ready" />Canal Shift <span>Lobby</span></button>
            <button onClick={() => onSelectTarget("rooms")}><i className="chip-live" />Observatory <span>Running</span></button>
            <button onClick={() => onSelectTarget("rooms")}><i className="chip-attention" />Courier Trial <span>Blocked</span></button>
          </div>
        </div>

        <section className="c-event-stream">
          <div className="c-stream-heading"><span>Live events</span><button onClick={() => onPreviewAction("Open the full event stream")}>Open stream ↗</button></div>
          <div className="c-events">
            <article><time>14:32:08</time><i className="event-good" /><span><strong>Canal Shift</strong> readiness changed to Ready</span><b>Task</b></article>
            <article><time>14:31:54</time><i className="event-blue" /><span><strong>Observatory Run</strong> committed Action at head 184</span><b>Room</b></article>
            <article><time>14:28:11</time><i className="event-warn" /><span><strong>atlas-runner-02</strong> missed its health window</span><b>Runner</b></article>
          </div>
        </section>
      </section>

      <aside className="c-inspector">
        <div className="c-inspector-top"><span>Inspector</span><button aria-label="Close inspector">×</button></div>
        <div className="c-inspector-identity"><div className={`c-target-icon target-${selected.status.toLowerCase().replaceAll(" ", "-")}`}>◇</div><span>{selected.type}</span><h2>{selected.label}</h2><StatusPill tone={selected.status === "Healthy" || selected.status === "Connected" ? "ready" : "attention"}>{selected.status}</StatusPill></div>
        <p className="c-inspector-detail">{selected.detail}</p>
        <div className="c-inspector-metrics">
          <div><span>Freshness</span><strong>Just now</strong></div>
          <div><span>Authority</span><strong>Bounded</strong></div>
          <div><span>Recovery</span><strong>Reconciled</strong></div>
        </div>
        <div className="c-inspector-actions">
          <button className="c-primary" onClick={() => onPreviewAction(`Open ${selected.label} details`)}>Open details</button>
          <button onClick={() => onPreviewAction(`Restart ${selected.label}`)}>Restart</button>
          <button onClick={() => onPreviewAction(`View ${selected.label} diagnostics`)}>Diagnostics</button>
        </div>
        <div className="c-impact">
          <span>Connected work</span>
          <article><TaskMark name="Canal Shift" /><div><strong>Canal Shift</strong><small>Ready to launch</small></div><b>›</b></article>
          <article><TaskMark name="Observatory Run" /><div><strong>Observatory Run</strong><small>Negotiation · live</small></div><b>›</b></article>
        </div>
        <div className="c-safety-note"><span>⌁</span><p><strong>Safe controls only</strong>The browser cannot run arbitrary commands or read raw credentials.</p></div>
      </aside>
    </div>
  );
}

interface HybridVariantProps extends TaskVariantProps {
  activeView: StudioArea;
  onChangeView: (view: StudioArea) => void;
  selectedTarget: string;
  onSelectTarget: (target: string) => void;
}

function VariantD({
  activeView,
  onChangeView,
  selectedTask,
  onSelectTask,
  selectedTarget,
  onSelectTarget,
  onPreviewAction,
}: HybridVariantProps) {
  const [taskMode, setTaskMode] = useState<TaskWorkspaceMode>("list");
  const [taskDetailTab, setTaskDetailTab] = useState<TaskDetailTab>("Overview");
  const [wizardStep, setWizardStep] = useState(1);
  const [buildArea, setBuildArea] = useState<BuildArea>("Activity Packs");
  const [operationsArea, setOperationsArea] = useState<OperationsArea>("Topology");

  const openTasks = (mode: TaskWorkspaceMode, taskId?: string) => {
    if (taskId) onSelectTask(taskId);
    setTaskMode(mode);
    onChangeView("Tasks");
  };

  const openOperations = (area: OperationsArea) => {
    setOperationsArea(area);
    onChangeView("Operations");
  };

  return (
    <div className="d-shell">
      <header className="d-header">
        <StudioBrand />
        <nav aria-label="Studio navigation" className="d-nav">
          <button className={activeView === "Home" ? "is-active" : ""} onClick={() => onChangeView("Home")}>Home</button>
          <button className={activeView === "Tasks" ? "is-active" : ""} onClick={() => openTasks("list")}>Tasks <span>4</span></button>
          <button className={activeView === "Build" ? "is-active" : ""} onClick={() => onChangeView("Build")}>Build</button>
          <button className={activeView === "Operations" ? "is-active" : ""} onClick={() => openOperations("Topology")}>Operations <i /></button>
          <button className={activeView === "Settings" ? "is-active" : ""} onClick={() => onChangeView("Settings")}>Settings</button>
        </nav>
        <div className="d-header-actions">
          <button className="d-search" aria-label="Search">⌕</button>
          <button className="d-health" onClick={() => openOperations("Attention")}><i /> System healthy <span>1 notice</span></button>
          <button className="d-new-task" onClick={() => openTasks("wizard")}>＋ New Task</button>
        </div>
      </header>

      {activeView === "Home" ? (
        <RecommendedHome
          onOpenOperations={openOperations}
          onOpenTasks={openTasks}
          onPreviewAction={onPreviewAction}
          onSelectTask={onSelectTask}
          selectedTask={selectedTask}
        />
      ) : null}
      {activeView === "Tasks" ? (
        <RecommendedTasks
          detailTab={taskDetailTab}
          mode={taskMode}
          onChangeDetailTab={setTaskDetailTab}
          onChangeMode={setTaskMode}
          onChangeWizardStep={setWizardStep}
          onPreviewAction={onPreviewAction}
          onSelectTask={onSelectTask}
          selectedTask={selectedTask}
          wizardStep={wizardStep}
        />
      ) : null}
      {activeView === "Build" ? (
        <RecommendedBuild
          activeArea={buildArea}
          onChangeArea={setBuildArea}
          onPreviewAction={onPreviewAction}
        />
      ) : null}
      {activeView === "Operations" ? (
        <RecommendedOperations
          activeArea={operationsArea}
          onChangeArea={setOperationsArea}
          onPreviewAction={onPreviewAction}
          onSelectTarget={onSelectTarget}
          selectedTarget={selectedTarget}
        />
      ) : null}
      {activeView === "Settings" ? <RecommendedSettings onPreviewAction={onPreviewAction} /> : null}
    </div>
  );
}

interface RecommendedHomeProps extends TaskVariantProps {
  onOpenTasks: (mode: TaskWorkspaceMode, taskId?: string) => void;
  onOpenOperations: (area: OperationsArea) => void;
}

function RecommendedHome({ selectedTask, onSelectTask, onOpenTasks, onOpenOperations, onPreviewAction }: RecommendedHomeProps) {
  const selected = tasks.find((task) => task.id === selectedTask) ?? tasks[0];
  return (
    <div className="d-page d-home">
      <section className="d-welcome">
        <div>
          <span className="d-overline">Your workspace · Sunday, August 23</span>
          <h1>Good afternoon. Here’s what needs you.</h1>
          <p>Start with the work. System details stay one step away in Operations.</p>
        </div>
        <div className="d-welcome-summary">
          <div><span>Active Tasks</span><strong>3</strong></div>
          <div><span>Ready to launch</span><strong>1</strong></div>
          <div><span>Runner capacity</span><strong>5 / 8</strong></div>
        </div>
      </section>

      <div className="d-home-grid">
        <section className="d-panel d-attention-panel">
          <div className="d-panel-heading">
            <div><span>Attention</span><h2>Three things to review</h2></div>
            <button onClick={() => onOpenOperations("Attention")}>View inbox →</button>
          </div>
          <div className="d-attention-list">
            {attentionItems.map((item, index) => (
              <button key={item.title} onClick={() => {
                onSelectTask(item.task);
                onPreviewAction(item.action);
              }}>
                <span className={`d-attention-icon d-level-${item.level.toLowerCase()}`}>{index + 1}</span>
                <span><strong>{item.title}</strong><small>{item.detail}</small></span>
                <span className="d-attention-action">{item.action} <b>→</b></span>
              </button>
            ))}
          </div>
        </section>

        <aside className="d-launch-card">
          <div className="d-launch-top"><span className="d-ready-icon">✓</span><StatusPill tone="ready">Ready to launch</StatusPill></div>
          <span className="d-card-kicker">Recommended Task</span>
          <h2>Canal Shift</h2>
          <p>All three seats are ready. The Activity is waiting safely in Lobby.</p>
          <div className="d-seat-stack" aria-label="Three ready seats"><i>VS</i><i>AI</i><i>AI</i><span>3 / 3 ready</span></div>
          <button onClick={() => onOpenTasks("detail", "canal-shift")}>Review & launch <b>→</b></button>
        </aside>
      </div>

      <section className="d-panel d-task-panel">
        <div className="d-panel-heading">
          <div><span>Tasks</span><h2>Current activity</h2></div>
          <div className="d-task-tools"><button className="is-active">Active</button><button>All</button><button onClick={() => onOpenTasks("list")}>Open Tasks ↗</button></div>
        </div>
        <div className="d-task-table" role="table" aria-label="Current Tasks">
          <div className="d-task-head" role="row"><span>Task</span><span>Phase</span><span>Status</span><span>People & agents</span><span>Runner</span><span /></div>
          {tasks.map((task) => (
            <button className={selectedTask === task.id ? "is-selected" : ""} key={task.id} onClick={() => onSelectTask(task.id)} role="row">
              <span><TaskMark name={task.name} /><span><strong>{task.name}</strong><small>{task.pack}</small></span></span>
              <span>{task.phase}</span>
              <span><StatusPill tone={task.tone}>{task.state}</StatusPill></span>
              <span>{task.seats}</span>
              <span>{task.runner}</span>
              <span>›</span>
            </button>
          ))}
        </div>
        <div className="d-selected-task" aria-live="polite"><span>Selected</span><strong>{selected.name}</strong><small>{selected.phase} · {selected.state} · {selected.updated}</small><button onClick={() => onOpenTasks("detail", selected.id)}>Open Task</button></div>
      </section>
    </div>
  );
}

interface RecommendedTasksProps extends TaskVariantProps {
  mode: TaskWorkspaceMode;
  detailTab: TaskDetailTab;
  wizardStep: number;
  onChangeMode: (mode: TaskWorkspaceMode) => void;
  onChangeDetailTab: (tab: TaskDetailTab) => void;
  onChangeWizardStep: (step: number) => void;
}

function RecommendedTasks({
  mode,
  detailTab,
  wizardStep,
  selectedTask,
  onSelectTask,
  onChangeMode,
  onChangeDetailTab,
  onChangeWizardStep,
  onPreviewAction,
}: RecommendedTasksProps) {
  const selected = tasks.find((task) => task.id === selectedTask) ?? tasks[0];

  if (mode === "wizard") {
    return (
      <TaskWizard
        onChangeMode={onChangeMode}
        onChangeStep={onChangeWizardStep}
        onPreviewAction={onPreviewAction}
        step={wizardStep}
      />
    );
  }

  if (mode === "detail") {
    return (
      <TaskDetail
        activeTab={detailTab}
        onBack={() => onChangeMode("list")}
        onChangeTab={onChangeDetailTab}
        onPreviewAction={onPreviewAction}
        task={selected}
      />
    );
  }

  return (
    <div className="d-page d-workspace-page">
      <section className="d-workspace-heading">
        <div><span className="d-overline">Tasks</span><h1>Every Room, in working language.</h1><p>Draft, prepare, launch, and follow a Task without handling credentials or Room IDs.</p></div>
        <button onClick={() => onChangeMode("wizard")}>＋ New Task</button>
      </section>

      <div className="d-workspace-toolbar">
        <div className="d-segmented"><button className="is-active">Active <span>3</span></button><button>Drafts <span>1</span></button><button>Completed <span>1</span></button></div>
        <div className="d-toolbar-actions"><button>⌕ Search</button><button>Filter⌄</button><button>List ▤</button></div>
      </div>

      <div className="d-task-workspace-grid">
        <section className="d-panel d-inventory-panel">
          <div className="d-inventory-head"><span>Task</span><span>Setup</span><span>Activity phase</span><span>Readiness</span><span>Freshness</span><span /></div>
          {tasks.map((task) => (
            <button key={task.id} onClick={() => { onSelectTask(task.id); onChangeMode("detail"); }}>
              <span><TaskMark name={task.name} /><span><strong>{task.name}</strong><small>{task.pack}</small></span></span>
              <span><StatusPill tone={task.tone === "attention" ? "attention" : "ready"}>{task.phase === "Setup" ? "Needs attention" : "Ready"}</StatusPill></span>
              <span><strong>{task.phase}</strong><small>{task.phase === "Lobby" ? "Waiting for launch" : "Generation current"}</small></span>
              <span>{task.seats}</span>
              <span>{task.updated}</span>
              <span>›</span>
            </button>
          ))}
        </section>

        <aside className="d-task-side">
          <section className="d-side-card d-draft-card">
            <span className="d-side-icon">✎</span><StatusPill tone="muted">Saved draft</StatusPill>
            <small>Last edited 18 min ago</small><h2>Night Market Trial</h2><p>Activity and configuration are complete. Continue with seat setup.</p>
            <div className="d-mini-progress"><span style={{ width: "42%" }} /></div>
            <button onClick={() => { onChangeWizardStep(3); onChangeMode("wizard"); }}>Resume draft →</button>
          </section>
          <section className="d-side-card">
            <span className="d-side-label">Task truth</span>
            <dl className="d-truth-list"><div><dt>Setup</dt><dd>Provisioning state</dd></div><div><dt>Activity phase</dt><dd>Pack-defined state</dd></div><div><dt>Integrity</dt><dd>Room health</dd></div><div><dt>Freshness</dt><dd>Observation age</dd></div></dl>
          </section>
        </aside>
      </div>
    </div>
  );
}

function TaskDetail({ task, activeTab, onBack, onChangeTab, onPreviewAction }: {
  task: (typeof tasks)[number];
  activeTab: TaskDetailTab;
  onBack: () => void;
  onChangeTab: (tab: TaskDetailTab) => void;
  onPreviewAction: (action: string) => void;
}) {
  const tabs: TaskDetailTab[] = ["Overview", "Setup", "Agent & MCP", "History"];
  return (
    <div className="d-page d-task-detail-page">
      <button className="d-back-link" onClick={onBack}>← All Tasks</button>
      <section className="d-detail-heading">
        <div><div className="d-detail-title"><TaskMark name={task.name} /><span><span className="d-overline">Agent Heist 1.1 · exact revision</span><h1>{task.name}</h1></span></div><div className="d-detail-badges"><StatusPill tone={task.tone}>{task.state}</StatusPill><span>Room integrity · Healthy</span><span>Fresh · 12 sec</span></div></div>
        <div className="d-detail-actions"><button onClick={() => onPreviewAction("Open replay")}>Replay</button><button onClick={() => onPreviewAction("Open bounded Task diagnostics")}>Diagnostics</button><button className="is-primary" onClick={() => onPreviewAction("Commit the idempotent Lobby launch input")}>Review & launch</button></div>
      </section>
      <nav className="d-detail-tabs" aria-label="Task detail views">{tabs.map((tab) => <button className={activeTab === tab ? "is-active" : ""} key={tab} onClick={() => onChangeTab(tab)}>{tab}{tab === "Agent & MCP" ? <i /> : null}</button>)}</nav>

      {activeTab === "Overview" ? <TaskOverview onPreviewAction={onPreviewAction} /> : null}
      {activeTab === "Setup" ? <TaskSetup onPreviewAction={onPreviewAction} /> : null}
      {activeTab === "Agent & MCP" ? <TaskAgentConnection onPreviewAction={onPreviewAction} /> : null}
      {activeTab === "History" ? <TaskHistory onPreviewAction={onPreviewAction} /> : null}
    </div>
  );
}

function TaskOverview({ onPreviewAction }: { onPreviewAction: (action: string) => void }) {
  return (
    <div className="d-detail-grid">
      <div className="d-detail-main">
        <section className="d-panel d-state-strip">
          <div><span>Setup</span><strong>Ready</strong><small>All authority provisioned</small></div><b>→</b><div><span>Activity phase</span><strong>Lobby</strong><small>No timer running</small></div><b>→</b><div><span>Room integrity</span><strong>Healthy</strong><small>Head 184 verified</small></div>
        </section>
        <section className="d-panel d-readiness-panel">
          <div className="d-panel-heading"><div><span>Launch readiness</span><h2>All required seats are ready</h2></div><StatusPill tone="ready">3 / 3 ready</StatusPill></div>
          <div className="d-seat-list">
            <article><span className="d-person human">VS</span><div><strong>Navigator</strong><small>Human · required</small></div><StatusPill tone="ready">Session ready</StatusPill><button onClick={() => onPreviewAction("Broker a seat-scoped Participant Console handoff")}>Open Participant View ↗</button></article>
            <article><span className="d-person agent">AI</span><div><strong>Insider</strong><small>External agent · required</small></div><StatusPill tone="ready">MCP connected</StatusPill><button onClick={() => onPreviewAction("Inspect Insider agent assignment")}>View assignment</button></article>
            <article><span className="d-person agent">AI</span><div><strong>Broker</strong><small>External agent · optional</small></div><StatusPill tone="ready">Runner ready</StatusPill><button onClick={() => onPreviewAction("Inspect Broker agent assignment")}>View assignment</button></article>
          </div>
        </section>
      </div>
      <aside className="d-detail-aside">
        <section className="d-side-card"><span className="d-side-label">Pinned build</span><dl className="d-key-values"><div><dt>Activity Pack</dt><dd>Agent Heist 1.1</dd></div><div><dt>Config digest</dt><dd>cfg_7b4…91e</dd></div><div><dt>Room</dt><dd>room_01K…8C2</dd></div><div><dt>Storage</dt><dd>SQLite durable</dd></div></dl></section>
        <section className="d-side-card d-operation-card"><span className="d-side-label">Creation operation</span><StatusPill tone="ready">Committed once</StatusPill><h3>create_01K…4DA</h3><p>Retries reconcile to the same Room. No replacement Room was created.</p><button onClick={() => onPreviewAction("Inspect the Room creation operation")}>Inspect operation</button></section>
      </aside>
    </div>
  );
}

function TaskSetup({ onPreviewAction }: { onPreviewAction: (action: string) => void }) {
  const stages = [
    ["Draft reviewed", "Exact Activity Pack revision and configuration pinned", "Complete"],
    ["Room created", "Idempotent operation resolved to room_01K…8C2", "Complete"],
    ["Memberships provisioned", "Three seats created with distinct participant authority", "Complete"],
    ["Runner authority assigned", "Two agent seats use separate Runner-control credentials", "Complete"],
    ["Readiness observed", "Required human session and compatible capacity are live", "Complete"],
  ];
  return <section className="d-panel d-setup-timeline"><div className="d-panel-heading"><div><span>Resumable setup</span><h2>Every stage is reconciled</h2></div><button onClick={() => onPreviewAction("Re-run setup reconciliation")}>Reconcile now</button></div><div>{stages.map(([title, detail, state], index) => <article key={title}><span>{index + 1}</span><div><strong>{title}</strong><small>{detail}</small></div><StatusPill tone="ready">{state}</StatusPill></article>)}</div><footer><span>✓</span><p><strong>Safe after restart</strong>Studio resumes from the last committed stage and never duplicates authority.</p></footer></section>;
}

function TaskAgentConnection({ onPreviewAction }: { onPreviewAction: (action: string) => void }) {
  return (
    <div className="d-agent-grid">
      <section className="d-panel d-agent-assignment">
        <div className="d-panel-heading"><div><span>Selected assignment</span><h2>Insider · External Planner r3</h2></div><StatusPill tone="ready">Connected</StatusPill></div>
        <div className="d-assignment-summary"><div><span>Membership</span><strong>member:insider/···14b2</strong></div><div><span>Agent Profile</span><strong>External Planner · r3</strong></div><div><span>Runner Template</span><strong>Local MCP Helper · r2</strong></div><div><span>Last observation ACK</span><strong>Head 184 · current</strong></div></div>
        <div className="d-tool-contract"><span>Assignment-bound MCP tools</span><article><b>01</b><div><strong>List assigned Tasks</strong><small>No arbitrary Room or Membership identifiers</small></div><StatusPill tone="ready">Available</StatusPill></article><article><b>02</b><div><strong>Observations + ACK</strong><small>Authorized projection and Cursor resume</small></div><StatusPill tone="ready">Head 184</StatusPill></article><article><b>03</b><div><strong>Exact Action Offers</strong><small>Schema and Room Head precondition included</small></div><StatusPill tone="ready">2 offers</StatusPill></article><article><b>04</b><div><strong>Next Activation + complete</strong><small>Separate Runner-control authority and lease</small></div><StatusPill tone="attention">1 leased</StatusPill></article></div>
      </section>
      <aside className="d-agent-side"><section className="d-side-card"><span className="d-side-label">Authority boundary</span><div className="d-authority-path"><span>Participant client<strong>Observations · Actions</strong></span><b>≠</b><span>Runner client<strong>Activations · completion</strong></span></div><p>Credentials and clients stay separate even when one helper exposes both tool groups.</p></section><section className="d-side-card"><span className="d-side-label">Connection</span><StatusPill tone="ready">stdio MCP ready</StatusPill><p>The helper starts already scoped to this assignment. Raw bearers never appear in Studio.</p><button onClick={() => onPreviewAction("Copy the non-secret MCP connection command")}>Copy connection command</button></section></aside>
    </div>
  );
}

function TaskHistory({ onPreviewAction }: { onPreviewAction: (action: string) => void }) {
  return <section className="d-panel d-history-panel"><div className="d-panel-heading"><div><span>Committed history</span><h2>Room timeline</h2></div><button onClick={() => onPreviewAction("Open verified replay")}>Open Replay ↗</button></div><ol><li><time>14:32:08</time><span><strong>Readiness changed to Ready</strong><small>All required seats observed</small></span><b>Head 184</b></li><li><time>14:31:54</time><span><strong>Observation acknowledged</strong><small>Insider external agent · Cursor current</small></span><b>Head 183</b></li><li><time>14:28:02</time><span><strong>Room entered Lobby</strong><small>No Activity timer started</small></span><b>Head 177</b></li><li><time>14:27:41</time><span><strong>Room creation committed</strong><small>Operation create_01K…4DA</small></span><b>Head 176</b></li></ol><footer><StatusPill tone="ready">Replay verified</StatusPill><span>Core, Activity, and aggregate hashes match committed history.</span></footer></section>;
}

function TaskWizard({ step, onChangeStep, onChangeMode, onPreviewAction }: {
  step: number;
  onChangeStep: (step: number) => void;
  onChangeMode: (mode: TaskWorkspaceMode) => void;
  onPreviewAction: (action: string) => void;
}) {
  const steps = ["Activity", "Configuration", "Seats", "Readiness", "Review"];
  const next = () => onChangeStep(Math.min(5, step + 1));
  const back = () => onChangeStep(Math.max(1, step - 1));
  return (
    <div className="d-page d-wizard-page">
      <button className="d-back-link" onClick={() => onChangeMode("list")}>← Tasks</button>
      <section className="d-wizard-heading"><div><span className="d-overline">Saved Task draft</span><h1>Create a new Task</h1><p>Nothing reaches the daemon until the final reviewed creation step.</p></div><span className="d-draft-state"><i /> Draft saved · just now</span></section>
      <div className="d-wizard-shell">
        <aside className="d-wizard-steps">{steps.map((label, index) => <button className={step === index + 1 ? "is-active" : step > index + 1 ? "is-complete" : ""} key={label} onClick={() => onChangeStep(index + 1)}><span>{step > index + 1 ? "✓" : index + 1}</span><span><strong>{label}</strong><small>{["Choose an exact pack", "Set Activity options", "Add people and agents", "Declare launch rules", "Confirm and create"][index]}</small></span></button>)}</aside>
        <section className="d-wizard-main"><div className="d-wizard-step-title"><span>Step {step} of 5</span><h2>{steps[step - 1]}</h2></div><WizardStepContent step={step} onPreviewAction={onPreviewAction} /><footer><button disabled={step === 1} onClick={back}>← Back</button><span>Changes stay in this draft</span>{step < 5 ? <button className="is-primary" onClick={next}>Continue →</button> : <button className="is-primary" onClick={() => { onPreviewAction("Create exactly one Room from the reviewed draft"); onChangeMode("detail"); }}>Create Task once</button>}</footer></section>
      </div>
    </div>
  );
}

function WizardStepContent({ step, onPreviewAction }: { step: number; onPreviewAction: (action: string) => void }) {
  if (step === 1) return <div className="d-wizard-content"><p>Select the exact installed Activity Pack revision. Studio will never silently upgrade it.</p><div className="d-pack-choice is-selected"><span className="d-side-icon">◇</span><div><strong>Agent Heist</strong><small>Revision 1.1 · Lobby launch compatible</small></div><StatusPill tone="ready">Installed</StatusPill></div><div className="d-pack-choice"><span className="d-side-icon">◇</span><div><strong>Agent Heist</strong><small>Revision 1.0 · immediate start</small></div><StatusPill tone="muted">Older</StatusPill></div><div className="d-schema-preview"><span>Declared by revision 1.1</span><b>Roles · Navigator, Insider, Broker</b><b>Actions · 9</b><b>Configuration fields · 4</b></div></div>;
  if (step === 2) return <div className="d-wizard-content"><p>These fields come from Agent Heist 1.1’s declared configuration schema.</p><div className="d-form-grid"><label>Task name<input defaultValue="Night Market Trial" /></label><label>Round duration<select defaultValue="8"><option value="8">8 minutes</option><option value="12">12 minutes</option></select></label><label>Maximum plans<input defaultValue="3" /></label><label className="d-checkbox"><input type="checkbox" defaultChecked /><span><strong>Require final acknowledgement</strong><small>All required participants acknowledge the outcome.</small></span></label></div></div>;
  if (step === 3) return <div className="d-wizard-content"><p>Assign a human or an exact Agent Profile revision to each declared Role.</p><div className="d-seat-builder"><article><span className="d-person human">VS</span><div><strong>Navigator</strong><small>Required Role</small></div><select defaultValue="human"><option value="human">Human participant</option></select><StatusPill tone="ready">Ready</StatusPill></article><article><span className="d-person agent">AI</span><div><strong>Insider</strong><small>Required Role</small></div><select defaultValue="external"><option value="external">External Planner · r3</option></select><StatusPill tone="ready">MCP</StatusPill></article><article><span className="d-person agent">AI</span><div><strong>Broker</strong><small>Optional Role</small></div><select defaultValue="managed"><option value="managed">Reference Agent Host · r1</option></select><StatusPill tone="attention">Secret needed</StatusPill></article></div><button className="d-inline-action" onClick={() => onPreviewAction("Create a new immutable Agent Profile revision")}>＋ Create Agent Profile</button></div>;
  if (step === 4) return <div className="d-wizard-content"><p>Choose what must be true before the operator can launch the Activity from Lobby.</p><div className="d-readiness-builder"><label><input type="checkbox" defaultChecked /><span><strong>Navigator session connected</strong><small>Required human seat has a usable Participant Console session.</small></span><StatusPill tone="ready">Required</StatusPill></label><label><input type="checkbox" defaultChecked /><span><strong>Insider runner capacity available</strong><small>Compatible runner has at least one free slot.</small></span><StatusPill tone="ready">Required</StatusPill></label><label><input type="checkbox" /><span><strong>Broker ready</strong><small>Optional seat can join later without blocking launch.</small></span><StatusPill tone="muted">Optional</StatusPill></label></div></div>;
  return <div className="d-wizard-content"><p>Review the exact build before creating one Room and provisioning its authority.</p><div className="d-review-grid"><section><span>Activity</span><strong>Agent Heist 1.1</strong><small>Lobby launch compatible</small></section><section><span>Configuration</span><strong>4 fields valid</strong><small>cfg_7b4…91e</small></section><section><span>Seats</span><strong>3 Roles</strong><small>2 required · 1 optional</small></section><section><span>Agents</span><strong>2 exact profiles</strong><small>External r3 · Managed r1</small></section></div><div className="d-create-guarantee"><span>1×</span><p><strong>Create exactly once</strong>A persisted operation is recorded before the daemon request. Retry resolves to the same Room.</p></div></div>;
}

function RecommendedBuild({ activeArea, onChangeArea, onPreviewAction }: {
  activeArea: BuildArea;
  onChangeArea: (area: BuildArea) => void;
  onPreviewAction: (action: string) => void;
}) {
  const areas: BuildArea[] = ["Activity Packs", "Agent Profiles", "Runner Templates", "Task Templates"];
  return (
    <div className="d-page d-build-page">
      <section className="d-workspace-heading"><div><span className="d-overline">Build</span><h1>Reusable parts, pinned exactly.</h1><p>Browse installed capabilities and create immutable revisions for repeatable Tasks.</p></div><button onClick={() => onPreviewAction(`Create a new ${activeArea.slice(0, -1)} revision`)}>＋ New revision</button></section>
      <nav className="d-area-tabs" aria-label="Build catalogs">{areas.map((area) => <button className={activeArea === area ? "is-active" : ""} key={area} onClick={() => onChangeArea(area)}>{area}<span>{({ "Activity Packs": 3, "Agent Profiles": 4, "Runner Templates": 3, "Task Templates": 2 } as Record<BuildArea, number>)[area]}</span></button>)}</nav>
      <BuildCatalog area={activeArea} onPreviewAction={onPreviewAction} />
    </div>
  );
}

function BuildCatalog({ area, onPreviewAction }: { area: BuildArea; onPreviewAction: (action: string) => void }) {
  const catalog: Record<BuildArea, Array<{ name: string; revision: string; kind: string; state: string; detail: string; meta: string[] }>> = {
    "Activity Packs": [
      { name: "Agent Heist", revision: "1.1", kind: "Activity Pack", state: "Installed", detail: "Lobby-gated collaborative planning Activity.", meta: ["3 Roles", "9 Actions", "Lobby compatible"] },
      { name: "Agent Heist", revision: "1.0", kind: "Activity Pack", state: "Retained", detail: "Original immediate-start semantic revision.", meta: ["3 Roles", "9 Actions", "Replay compatible"] },
      { name: "Counter", revision: "1.0", kind: "Activity Pack", state: "Installed", detail: "Minimal deterministic reference Activity.", meta: ["1 Role", "1 Action", "Conformance"] },
    ],
    "Agent Profiles": [
      { name: "External Planner", revision: "r3", kind: "External MCP", state: "Ready", detail: "Assignment-bound external agent with no model execution in WorldStream.", meta: ["2 assignments", "Secret configured", "Immutable"] },
      { name: "Reference Agent Host", revision: "r1", kind: "Managed host", state: "Needs secret", detail: "Post-MVP reference host using the same bounded MCP contract.", meta: ["0 assignments", "Provider missing", "Immutable"] },
      { name: "Observer", revision: "r2", kind: "External MCP", state: "Ready", detail: "Observation and ACK only; no participant Action submission.", meta: ["1 assignment", "Read only", "Immutable"] },
    ],
    "Runner Templates": [
      { name: "Local MCP Helper", revision: "r2", kind: "Approved process", state: "Running", detail: "Generic local stdio MCP bridge for external agents.", meta: ["Agent Heist 1.x", "4 capacity", "Healthy"] },
      { name: "Atlas Runner", revision: "r4", kind: "Approved process", state: "Degraded", detail: "Owner-installed Runner with one unavailable instance.", meta: ["Agent Heist 1.1", "4 capacity", "1 offline"] },
      { name: "Reference Agent Host", revision: "r1", kind: "Approved process", state: "Stopped", detail: "Managed reference model host, outside worldstreamd.", meta: ["Post-MVP", "1 capacity", "Secret required"] },
    ],
    "Task Templates": [
      { name: "Agent Heist · Human + two agents", revision: "r3", kind: "Task Template", state: "Ready", detail: "Standard three-seat Lobby configuration.", meta: ["Pack 1.1", "3 exact profiles", "2 required"] },
      { name: "Agent Heist · Human-led", revision: "r1", kind: "Task Template", state: "Ready", detail: "Two humans and one optional external agent.", meta: ["Pack 1.1", "1 exact profile", "Optional Broker"] },
    ],
  };
  const selected = catalog[area][0];
  return <div className="d-build-grid"><section className="d-panel d-catalog-list"><div className="d-catalog-toolbar"><button>⌕ Search {area.toLowerCase()}</button><button>Compatible⌄</button></div>{catalog[area].map((item, index) => <button className={index === 0 ? "is-selected" : ""} key={`${item.name}-${item.revision}`}><span className="d-side-icon">◇</span><span><small>{item.kind}</small><strong>{item.name}</strong><p>{item.detail}</p><span className="d-catalog-meta">{item.meta.map((value) => <b key={value}>{value}</b>)}</span></span><span><StatusPill tone={item.state === "Ready" || item.state === "Installed" || item.state === "Running" ? "ready" : item.state === "Degraded" || item.state === "Needs secret" ? "attention" : "muted"}>{item.state}</StatusPill><strong>{item.revision}</strong></span></button>)}</section><aside className="d-side-card d-catalog-detail"><span className="d-side-label">Exact selected revision</span><span className="d-detail-glyph">◇</span><small>{selected.kind}</small><h2>{selected.name} · {selected.revision}</h2><p>{selected.detail}</p><dl className="d-key-values">{selected.meta.map((value, index) => <div key={value}><dt>{["Capability", "Assignment", "Compatibility"][index] ?? "Detail"}</dt><dd>{value}</dd></div>)}</dl><div className="d-catalog-actions"><button onClick={() => onPreviewAction(`Inspect ${selected.name} ${selected.revision}`)}>Inspect</button><button className="is-primary" onClick={() => onPreviewAction(`Create a successor revision from ${selected.name} ${selected.revision}`)}>Create successor</button></div><p className="d-immutability-note"><strong>Published revisions are immutable.</strong> Changes always create a new exact revision.</p></aside></div>;
}

function RecommendedOperations({ selectedTarget, onSelectTarget, onPreviewAction, activeArea, onChangeArea }: TopologyVariantProps & {
  activeArea: OperationsArea;
  onChangeArea: (area: OperationsArea) => void;
}) {
  const targets: Record<string, { label: string; type: string; status: string; tone: string; detail: string }> = {
    supervisor: { label: "Studio Supervisor", type: "Local control plane", status: "Healthy", tone: "ready", detail: "Serving Studio, protecting credentials, and reconciling four Tasks." },
    worldstreamd: { label: "worldstreamd", type: "Authoritative Room runtime", status: "Healthy", tone: "ready", detail: "v0.1.0 · SQLite durable · 3h 12m uptime · live status." },
    rooms: { label: "Task Rooms", type: "4 WorldStream Rooms", status: "1 notice", tone: "attention", detail: "One Room is in setup, one in Lobby, one running, and one complete." },
    runners: { label: "Runner fleet", type: "3 approved runners", status: "Degraded", tone: "attention", detail: "5 of 8 slots are in use. atlas-runner-02 is unavailable." },
    mcp: { label: "External MCP", type: "Assignment-bound agent path", status: "Connected", tone: "ready", detail: "Two agent assignments are connected and their observation ACKs are current." },
  };
  const selected = targets[selectedTarget] ?? targets.worldstreamd;

  return (
    <div className="d-page d-operations">
      <section className="d-ops-heading">
        <div><span className="d-overline">Operations</span><h1>System status without the noise.</h1><p>See what is connected, what is stale, and which safe action is available.</p></div>
        <div className="d-live-state"><i /> Live · updated just now</div>
      </section>

      <nav className="d-area-tabs d-ops-tabs" aria-label="Operations views">
        {(["Topology", "Processes", "Storage", "Attention"] as OperationsArea[]).map((area) => (
          <button className={activeArea === area ? "is-active" : ""} key={area} onClick={() => onChangeArea(area)}>{area}{area === "Attention" ? <span>3</span> : null}</button>
        ))}
      </nav>

      {activeArea === "Topology" ? (
        <>
          <div className="d-ops-summary">
            <button className={selectedTarget === "worldstreamd" ? "is-selected" : ""} onClick={() => onSelectTarget("worldstreamd")}><span><i className="d-good" />Daemon</span><strong>Healthy</strong><small>v0.1.0 · 3h 12m</small></button>
            <button className={selectedTarget === "runners" ? "is-selected" : ""} onClick={() => onSelectTarget("runners")}><span><i className="d-warn" />Runner fleet</span><strong>5 / 8 capacity</strong><small>1 runner unavailable</small></button>
            <button onClick={() => onChangeArea("Storage")}><span><i className="d-good" />Storage</span><strong>Verified</strong><small>Backup · 3 days ago</small></button>
            <button className={selectedTarget === "rooms" ? "is-selected" : ""} onClick={() => onSelectTarget("rooms")}><span><i className="d-good" />Rooms</span><strong>4 known</strong><small>3 active · 1 complete</small></button>
          </div>

          <div className="d-ops-grid">
            <section className="d-topology-panel">
              <div className="d-topology-heading"><div><span>Live topology</span><h2>How the local system connects</h2></div><div><button>−</button><b>100%</b><button>＋</button><button>Center</button></div></div>
              <div className="d-topology" aria-label="Recommended Operations topology">
                <svg viewBox="0 0 760 330" preserveAspectRatio="none" aria-hidden="true">
                  <path d="M 150 165 C 205 165, 205 165, 270 165" />
                  <path d="M 410 165 C 465 165, 460 82, 520 82" />
                  <path d="M 410 165 C 465 165, 460 250, 520 250" />
                  <path d="M 640 250 C 690 250, 690 165, 690 165" />
                </svg>
                <HybridNode className="d-node-supervisor" label="Supervisor" detail="Control plane" state="Healthy" tone="ready" selected={selectedTarget === "supervisor"} onClick={() => onSelectTarget("supervisor")} />
                <HybridNode className="d-node-daemon" label="worldstreamd" detail="Room runtime" state="Healthy" tone="ready" selected={selectedTarget === "worldstreamd"} onClick={() => onSelectTarget("worldstreamd")} featured />
                <HybridNode className="d-node-rooms" label="4 Task Rooms" detail="3 active" state="1 notice" tone="attention" selected={selectedTarget === "rooms"} onClick={() => onSelectTarget("rooms")} />
                <HybridNode className="d-node-runners" label="Runner fleet" detail="5 / 8 capacity" state="Degraded" tone="attention" selected={selectedTarget === "runners"} onClick={() => onSelectTarget("runners")} />
                <HybridNode className="d-node-mcp" label="External MCP" detail="2 assignments" state="Connected" tone="ready" selected={selectedTarget === "mcp"} onClick={() => onSelectTarget("mcp")} />
                <div className="d-room-list"><span><i className="d-good" />Canal Shift <b>Lobby</b></span><span><i className="d-good" />Observatory <b>Running</b></span><span><i className="d-warn" />Courier Trial <b>Setup</b></span></div>
              </div>
            </section>

            <aside className="d-inspector">
              <div className="d-inspector-heading"><span>Selected system</span><StatusPill tone={selected.tone}>{selected.status}</StatusPill></div>
              <span className="d-inspector-icon">◇</span>
              <small>{selected.type}</small>
              <h2>{selected.label}</h2>
              <p>{selected.detail}</p>
              <dl><div><dt>Freshness</dt><dd>Just now</dd></div><div><dt>Authority</dt><dd>Bounded</dd></div><div><dt>Recovery</dt><dd>Reconciled</dd></div></dl>
              <button className="d-inspector-primary" onClick={() => onPreviewAction(`Open ${selected.label} details`)}>Open details</button>
              <div className="d-inspector-actions"><button onClick={() => onPreviewAction(`Restart ${selected.label}`)}>Restart</button><button onClick={() => onPreviewAction(`Inspect ${selected.label} diagnostics`)}>Diagnostics</button></div>
              <div className="d-safe-controls"><span>⌁</span><p><strong>Safe controls only</strong>No arbitrary commands or raw credentials.</p></div>
            </aside>
          </div>
        </>
      ) : null}
      {activeArea === "Processes" ? <OperationsProcesses onPreviewAction={onPreviewAction} /> : null}
      {activeArea === "Storage" ? <OperationsStorage onPreviewAction={onPreviewAction} /> : null}
      {activeArea === "Attention" ? <OperationsAttention onPreviewAction={onPreviewAction} /> : null}
    </div>
  );
}

function OperationsProcesses({ onPreviewAction }: { onPreviewAction: (action: string) => void }) {
  return (
    <div className="d-ops-workspace">
      <section className="d-panel d-process-panel">
        <div className="d-panel-heading"><div><span>Registered daemon</span><h2>worldstreamd</h2></div><StatusPill tone="ready">Running</StatusPill></div>
        <div className="d-process-summary"><div><span>Lifecycle</span><strong>Running</strong><small>PID 48211</small></div><div><span>Version</span><strong>v0.1.0</strong><small>Compatible</small></div><div><span>Uptime</span><strong>3h 12m</strong><small>Started 11:20</small></div><div><span>Recovery</span><strong>Reconciled</strong><small>4 Rooms known</small></div></div>
        <div className="d-lifecycle-actions"><button disabled>Start</button><button onClick={() => onPreviewAction("Gracefully stop worldstreamd")}>Graceful stop</button><button className="is-primary" onClick={() => onPreviewAction("Restart worldstreamd through typed lifecycle control")}>Restart</button><span>Only typed operations for the registered daemon are available.</span></div>
      </section>
      <section className="d-panel d-runner-panel">
        <div className="d-panel-heading"><div><span>Approved processes</span><h2>Runner instances</h2></div><button onClick={() => onPreviewAction("Start an approved Runner Template revision")}>＋ Start runner</button></div>
        <div className="d-runner-table"><div className="d-runner-head"><span>Instance</span><span>Template</span><span>Compatibility</span><span>Capacity</span><span>Freshness</span><span /></div><article><span><i className="d-good" /><strong>local-mcp-01</strong></span><span>Local MCP Helper · r2</span><span>Agent Heist 1.x</span><span>2 / 4</span><span>Just now</span><button onClick={() => onPreviewAction("Restart local-mcp-01")}>Restart</button></article><article><span><i className="d-warn" /><strong>atlas-runner-02</strong></span><span>Atlas Runner · r4</span><span>Agent Heist 1.1</span><span>3 / 4</span><span>4 min stale</span><button onClick={() => onPreviewAction("Retry atlas-runner-02")}>Retry</button></article><article><span><i className="d-good" /><strong>reference-host-01</strong></span><span>Reference Agent Host · r1</span><span>Agent Heist 1.1</span><span>0 / 1</span><span>Stopped</span><button onClick={() => onPreviewAction("Start reference-host-01")}>Start</button></article></div>
      </section>
      <section className="d-panel d-activation-panel"><div className="d-panel-heading"><div><span>Activation attention</span><h2>Bounded queue status</h2></div><span className="d-private-note">Private payloads hidden</span></div><div className="d-activation-summary"><article><strong>2</strong><span>Waiting</span><small>Oldest · 18 sec</small></article><article><strong>1</strong><span>Leased</span><small>Expires · 42 sec</small></article><article><strong>0</strong><span>Failed</span><small>No intervention</small></article><article><strong>184</strong><span>Latest ACK</span><small>Cursor current</small></article></div></section>
    </div>
  );
}

function OperationsStorage({ onPreviewAction }: { onPreviewAction: (action: string) => void }) {
  return (
    <div className="d-storage-grid">
      <section className="d-panel d-storage-main"><div className="d-panel-heading"><div><span>Active storage profile</span><h2>SQLite durable</h2></div><StatusPill tone="ready">Verified</StatusPill></div><div className="d-storage-health"><span className="d-storage-icon">▱</span><div><span>Integrity verification</span><strong>Healthy</strong><small>Last verified 22 seconds ago · daemon supported check</small></div></div><dl className="d-storage-facts"><div><dt>Profile</dt><dd>sqlite-durable</dd></div><div><dt>Database size</dt><dd>84.2 MB</dd></div><div><dt>Known Rooms</dt><dd>4</dd></div><div><dt>Last complete head</dt><dd>184</dd></div></dl><div className="d-backup-callout"><div><span>Last verified backup</span><strong>August 20 · 14:02</strong><small>worldstream-backup-2026-08-20 · 82.9 MB</small></div><StatusPill tone="attention">3 days old</StatusPill><button onClick={() => onPreviewAction("Create one safe verified storage backup")}>Create backup</button></div><div className="d-backup-progress"><span>Backup operation · idle</span><div><i style={{ width: "0%" }} /></div><small>No backup is currently running.</small></div></section>
      <aside className="d-storage-side"><section className="d-side-card"><span className="d-side-label">Backup history</span><article><span><i className="d-good" />Aug 20 · 14:02</span><strong>Verified</strong><small>82.9 MB</small></article><article><span><i className="d-good" />Aug 17 · 09:31</span><strong>Verified</strong><small>79.1 MB</small></article><button onClick={() => onPreviewAction("Inspect bounded backup history")}>View all backups</button></section><section className="d-side-card d-offline-note"><span>!</span><p><strong>Offline recovery only</strong>Restore, migrate, reset, and repair are intentionally unavailable in Studio.</p></section></aside>
    </div>
  );
}

function OperationsAttention({ onPreviewAction }: { onPreviewAction: (action: string) => void }) {
  return (
    <div className="d-attention-workspace">
      <section className="d-panel d-inbox-panel"><div className="d-panel-heading"><div><span>Active inbox</span><h2>Three conditions need attention</h2></div><div className="d-inbox-filters"><button className="is-active">Open</button><button>Resolved</button></div></div>{attentionItems.map((item, index) => <article key={item.title}><span className={`d-attention-icon d-level-${item.level.toLowerCase()}`}>{index + 1}</span><div><div><span>{item.level} priority</span><time>{["4 min ago", "3 days ago", "12 min ago"][index]}</time></div><h3>{item.title}</h3><p>{item.detail}</p><small>Affected · {index === 0 ? "Courier Trial / Insider" : index === 1 ? "SQLite durable" : "Reference Agent Host r1"}</small></div><button onClick={() => onPreviewAction(item.action)}>{item.action} →</button></article>)}</section>
      <aside className="d-attention-side"><section className="d-side-card"><span className="d-side-label">Local notifications</span><div className="d-toggle-row"><div><strong>Operating-system notifications</strong><small>Notify only when a new actionable condition appears.</small></div><label className="d-switch"><input type="checkbox" defaultChecked /><span /></label></div><div className="d-toggle-row"><div><strong>Runner capacity loss</strong><small>Only when it blocks a required seat.</small></div><label className="d-switch"><input type="checkbox" defaultChecked /><span /></label></div><div className="d-toggle-row"><div><strong>Backup reminders</strong><small>After three days without a verified backup.</small></div><label className="d-switch"><input type="checkbox" defaultChecked /><span /></label></div></section><section className="d-side-card"><span className="d-side-label">Notification safety</span><p>Messages explain why they appeared, contain no credentials or private participant payloads, and never mark the underlying condition resolved.</p></section></aside>
    </div>
  );
}

function RecommendedSettings({ onPreviewAction }: { onPreviewAction: (action: string) => void }) {
  const credentials = [
    ["Host authority", "Configured", "Controls Room creation and host inputs", "ready"],
    ["Membership credential store", "7 references", "Seat-scoped participant authority", "ready"],
    ["Runner authority store", "3 references", "Activation leases and completion", "ready"],
    ["Model provider", "Not configured", "Optional · used only by managed Agent Host", "attention"],
  ];
  return (
    <div className="d-page d-settings-page">
      <section className="d-workspace-heading"><div><span className="d-overline">Settings</span><h1>Local, protected, and deliberately small.</h1><p>No user login is required. Studio holds a local privileged session and never sends raw bearers to the browser.</p></div><StatusPill tone="ready">Local session active</StatusPill></section>
      <div className="d-settings-grid"><div><section className="d-panel d-settings-section"><div className="d-panel-heading"><div><span>Protected references</span><h2>Credentials</h2></div><span className="d-private-note">Values never displayed</span></div><div className="d-credential-list">{credentials.map(([name, state, detail, tone]) => <article key={name}><span className="d-credential-icon">⌁</span><div><strong>{name}</strong><small>{detail}</small></div><StatusPill tone={tone}>{state}</StatusPill><button onClick={() => onPreviewAction(`${state === "Not configured" ? "Configure" : "Replace"} ${name}`)}>{state === "Not configured" ? "Configure" : "Replace"}</button></article>)}</div></section><section className="d-panel d-settings-section"><div className="d-panel-heading"><div><span>Runtime registration</span><h2>Local services</h2></div></div><div className="d-service-settings"><article><i className="d-good" /><div><strong>worldstreamd</strong><small>Registered daemon · v0.1.0</small></div><code>local service reference · daemon-primary</code><button onClick={() => onPreviewAction("Edit bounded daemon registration")}>Edit</button></article><article><i className="d-good" /><div><strong>Studio Supervisor</strong><small>Control-plane database · healthy</small></div><code>studio-control.sqlite</code><button onClick={() => onPreviewAction("Inspect Supervisor storage")}>Inspect</button></article></div></section></div><aside><section className="d-side-card d-local-session"><span className="d-side-icon">✓</span><span className="d-side-label">Local session</span><h2>No login screen</h2><p>The Supervisor establishes the browser’s privileged local session. Participant sessions are always brokered separately.</p></section><section className="d-side-card"><span className="d-side-label">Browser safety</span><dl className="d-key-values"><div><dt>Raw bearers</dt><dd>Never returned</dd></div><div><dt>Arbitrary commands</dt><dd>Unavailable</dd></div><div><dt>Private Invocations</dt><dd>Hidden</dd></div><div><dt>Agent memory</dt><dd>External</dd></div></dl></section></aside></div>
    </div>
  );
}

function HybridNode({ className, label, detail, state, tone, selected, onClick, featured = false }: {
  className: string;
  label: string;
  detail: string;
  state: string;
  tone: string;
  selected: boolean;
  onClick: () => void;
  featured?: boolean;
}) {
  return (
    <button className={`d-node ${className} ${selected ? "is-selected" : ""} ${featured ? "is-featured" : ""}`} onClick={onClick}>
      <span>◇</span><span><strong>{label}</strong><small>{detail}</small></span><StatusPill tone={tone}>{state}</StatusPill>
    </button>
  );
}

function PrototypeSwitcher({ current, onChange, onCycle, hybridView, selectedTask, selectedTarget }: {
  current: VariantKey;
  onChange: (variant: VariantKey) => void;
  onCycle: (direction: number) => void;
  hybridView: StudioArea;
  selectedTask: string;
  selectedTarget: string;
}) {
  const active = variants.find((variant) => variant.key === current)!;
  return (
    <aside className="prototype-switcher" aria-label="Prototype variant switcher">
      <button className="switch-arrow" aria-label="Previous variant" onClick={() => onCycle(-1)}>←</button>
      <div className="switch-copy">
        <span><strong>{active.key}</strong> — {active.name}</span>
        <small>Static fixture · View: {hybridView} · Task: {selectedTask} · Focus: {selectedTarget}</small>
      </div>
      <div className="switch-keys" role="group" aria-label="Choose a variant">
        {variants.map((variant) => <button className={variant.key === current ? "is-active" : ""} key={variant.key} onClick={() => onChange(variant.key)}>{variant.key}</button>)}
      </div>
      <button className="switch-arrow" aria-label="Next variant" onClick={() => onCycle(1)}>→</button>
    </aside>
  );
}

function StudioBrand({ compact = false }: { compact?: boolean }) {
  return (
    <div className={`studio-brand ${compact ? "is-compact" : ""}`}>
      <span className="studio-brand-mark" aria-hidden="true"><i /><b /></span>
      <span><strong>WorldStream</strong><small>Studio</small></span>
    </div>
  );
}

function NavButton({ glyph, label, active = false, count, attention = false }: { glyph: string; label: string; active?: boolean; count?: string; attention?: boolean }) {
  return <button className={active ? "is-active" : ""}><span>{glyph}</span><strong>{label}</strong>{count ? <b className={attention ? "is-attention" : ""}>{count}</b> : null}</button>;
}

function Metric({ label, value, detail, tone }: { label: string; value: string; detail: string; tone: string }) {
  return <article className={`metric-card metric-${tone}`}><span>{label}</span><strong>{value}</strong><small>{detail}</small><i /></article>;
}

function TaskMark({ name }: { name: string }) {
  return <span className="task-mark" aria-hidden="true">{name.split(" ").map((part) => part[0]).join("").slice(0, 2)}</span>;
}

function StatusPill({ tone, children }: { tone: string; children: string }) {
  return <span className={`studio-status status-${tone}`}><i />{children}</span>;
}

function TopologyNode({ className, label, meta, status, tone, selected, onClick, featured = false }: {
  className: string;
  label: string;
  meta: string;
  status: string;
  tone: string;
  selected: boolean;
  onClick: () => void;
  featured?: boolean;
}) {
  return (
    <button className={`c-node ${className} ${selected ? "is-selected" : ""} ${featured ? "is-featured" : ""}`} onClick={onClick}>
      <span className="c-node-icon">◇</span>
      <span><strong>{label}</strong><small>{meta}</small></span>
      <StatusPill tone={tone}>{status}</StatusPill>
    </button>
  );
}
