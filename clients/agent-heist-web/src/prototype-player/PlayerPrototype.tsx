// Guided practice uses Mission focus only; development retains the layout study.
// Question: can a first-time human understand, act and explain the result without typing IDs?
// All state is a scripted, untimed practice sample. No sessions, providers, API calls or Room truth.
import { useEffect, useRef, useState, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import rooftop from "../assets/heist-rooftop.webp";
import { HeistArtwork, type HeistArtworkKind } from "../HeistArtwork";
import "./prototype.css";

type Variant = "A" | "B" | "C";
type Step = 0 | 1 | 2 | 3 | 4 | 5;
const variants: Variant[] = ["A", "B", "C"];
const names = { A: "Mission focus", B: "Crew tabletop", C: "Interactive story" };
const artKinds: HeistArtworkKind[] = ["route", "entry_window", "required_tool", "extraction"];
const artValues = [["canal", "service", "rooftop"], ["late", "early", "middle"], ["disguise", "thermal_key", "jammer"], ["van", "boat", "motorbike"]];
function ChoiceArt({ part, option, className = "" }: { part: number; option: number; className?: string }) {
  return <HeistArtwork kind={artKinds[part]!} value={artValues[part]?.[option] ?? "unknown"} className={className} />;
}
const chapters = ["Safehouse", "Discover", "Share", "Build a plan", "Lock it in", "Debrief"];
const choices = [
  { label: "Way in", hint: "Your route intel says: Service entrance.", options: ["Canal", "Service entrance", "Rooftop"], correct: 1, symbol: "↳" },
  { label: "When to enter", hint: "Insider shared: Enter early.", options: ["Late", "Early", "Middle window"], correct: 1, symbol: "◷" },
  { label: "Equipment", hint: "Broker shared: Bring a thermal key.", options: ["Disguise", "Thermal key", "Jammer"], correct: 1, symbol: "⌘" },
  { label: "Way out", hint: "Broker shared: Leave by boat.", options: ["Van", "Boat", "Motorbike"], correct: 1, symbol: "⇢" },
];
const objectives = [
  ["One crew. One way out.", "Find four pieces of intel. Agree on a plan. Commit together."],
  ["Find your way in.", "You are the Navigator. Your sealed dossier contains the route the crew needs."],
  ["A secret does not help the crew.", "You know the way in. Decide whether to share your intel with everyone."],
  ["Build the crew's plan.", "Choose a way in, a time, a tool and a way out. Then the crew must commit to the same complete plan."],
  ["Make your choice count.", "Backing a plan is not your final vote. Seal your choice before the window closes."],
  ["Every choice left a trace.", "This is a crew result, not an individual ranking. Here is how this practice plan scored."],
];

export type PlayerPrototypeProps = { practiceMode?: boolean; onExitPractice?: () => void };

export function mountPrototype(props: PlayerPrototypeProps = {}) {
  document.title = "Agent Heist · Player experience study";
  createRoot(document.getElementById("root")!).render(<PlayerPrototype {...props} />);
}

export function PlayerPrototype({ practiceMode = false, onExitPractice }: PlayerPrototypeProps) {
  const requested = practiceMode ? null : new URLSearchParams(location.search).get("variant");
  const requestedStep = practiceMode ? 0 : Number(new URLSearchParams(location.search).get("step") ?? 0);
  const initialStep = (Number.isInteger(requestedStep) && requestedStep >= 0 && requestedStep <= 5 ? requestedStep : 0) as Step;
  const [variant, setVariant] = useState<Variant>(practiceMode ? "A" : variants.includes(requested as Variant) ? requested as Variant : "A");
  const [step, setStep] = useState<Step>(initialStep);
  const [inspected, setInspected] = useState(initialStep > 1);
  const [shared, setShared] = useState(initialStep > 2);
  const [part, setPart] = useState(0);
  const [plan, setPlan] = useState([1, 1, 1, 1]);
  const [visited, setVisited] = useState<number[]>(initialStep > 3 ? [0,1,2,3] : []);
  const [resource, setResource] = useState(true);
  const [drawer, setDrawer] = useState<null | "rules" | "intel" | "crew" | "state">(null);
  const [tour, setTour] = useState(true);
  const [notice, setNotice] = useState("Practice is untimed. No live room or AI calls.");
  const dialog = useRef<HTMLDialogElement>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const checks = choices.map((choice, i) => plan[i] === choice.correct);
  const score = checks.filter(Boolean).length + Number(resource);
  const phase = step === 0 ? "Pre-game" : step === 1 ? "Briefing" : step < 4 ? "Negotiation" : step === 4 ? "Commitment" : "Result";
  function switchVariant(next: Variant) {
    if (practiceMode) return;
    setVariant(next);
    const url = new URL(location.href); url.searchParams.set("variant", next);
    history.replaceState(null, "", url);
  }
  function cycle(offset: number) { switchVariant(variants[(variants.indexOf(variant) + offset + 3) % 3]!); }
  function advance(next: Step) { setStep(next); setDrawer(null); }
  function reset() { setStep(0); setInspected(false); setShared(false); setPart(0); setVisited([]); setPlan([1, 1, 1, 1]); setResource(true); setNotice("Practice reset. No live room or AI calls."); }
  useEffect(() => {
    if (practiceMode) return;
    const key = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement;
      if (target.closest("input,textarea,select,[contenteditable=true],[role=tablist],dialog")) return;
      if (event.key === "ArrowLeft" || event.key === "ArrowRight") { event.preventDefault(); cycle(event.key === "ArrowLeft" ? -1 : 1); }
    };
    window.addEventListener("keydown", key); return () => window.removeEventListener("keydown", key);
  }, [practiceMode, variant]);
  useEffect(() => { if (drawer) dialog.current?.showModal(); else dialog.current?.close(); }, [drawer]);
  useEffect(() => { heading.current?.focus(); }, [step]);
  const openIntel = () => setDrawer("intel");
  const primary = (label: string, action: () => void, disabled = false) => <button className="px-primary" disabled={disabled} onClick={action}>{label}<span aria-hidden="true">↗</span></button>;

  const feedback = <div className="px-feedback" role="status">{notice}</div>;
  const coach = tour && step < 5 ? <aside className="px-coach"><span className="px-coach-icon">?</span><div><b>{["First heist? You are in the right place.", "Start with what only you know.", "Private until you share it.", "You are choosing, not filling a form.", "This is the decision that is scored."][step]}</b><p>{[
    "Try a short practice first. In a live game the crew shares one clock; opening help does not pause it.",
    "Open the dossier to read your clue. Nobody else can see it yet. There is no code to find or type.",
    "Share with crew makes this clue public. You can also keep intel private or trade it with one teammate in a live game.",
    "Use the intel you have. If nobody has shared an answer, you will need to ask, trade, or take a risk.",
    "At least two crew members must choose the same plan. Your sealed choice stays private until resolution.",
  ][step]}</p></div><button aria-label="Hide guidance" className="px-icon" onClick={() => setTour(false)}>×</button></aside> : null;

  let play: ReactNode;
  if (step === 0) play = <section className="px-onboard">
    <span className="px-overline">YOUR ROLE / NAVIGATOR</span>
    <h2>You know the way in.</h2><p>Your teammates know the timing, equipment and escape. Combine your intel before the crew commits.</p>
    <div className="px-how"><span><b>01</b> Open your intel</span><span><b>02</b> Build a crew plan</span><span><b>03</b> Seal your choice</span></div>
    <div className="px-score-note"><strong>The goal: 5 / 5</strong><span>Four correct plan details + one resource contribution from a supporter of the selected plan.</span></div>
    {primary("Try the practice heist", () => advance(1))}
    <button className="px-text-button" onClick={() => setDrawer("rules")}>How do we win?</button>
  </section>;
  else if (step === 1) play = <section className={`px-dossier ${inspected ? "opened" : ""}`}>
    <div className="px-dossier-top"><span>{inspected ? "PRIVATE / ONLY YOU" : "SEALED / YOUR INTEL"}</span><span>01</span></div>
    {inspected ? <ChoiceArt part={0} option={1} className="px-dossier-art" /> : <div className="px-dossier-mark" aria-hidden="true">◇</div>}
    <h2>{inspected ? "The service entrance." : "Route dossier"}</h2>
    <p>{inspected ? "Your intel confirms that the service entrance is the correct way in. Your crew does not know this yet." : "There is one route the crew needs to find. Open your dossier to learn it."}</p>
    {inspected ? primary("Continue practice → planning", () => advance(2)) : primary("Open my dossier", () => { setInspected(true); setNotice("Dossier opened. This clue is still private."); })}
    <span className="px-fine">{inspected ? "Practice phase jump. Live phases follow the shared clock." : "The contents stay private until you choose to share them."}</span>
  </section>;
  else if (step === 2) play = <section className="px-share">
    <span className="px-overline">{shared ? "SHARED WITH THE CREW" : "YOUR PRIVATE INTEL"}</span>
    <div className="px-intel-line"><ChoiceArt part={0} option={1} className="px-intel-thumb" /><div><small>Way in</small><h2>Service entrance</h2></div><span className="px-seal">{shared ? "✓ Shared" : "Only you"}</span></div>
    <p>{shared ? "The crew can now use your route. In this practice, your teammates have shared their intel too." : "Give everyone the route you found. It will appear on the shared crew board."}</p>
    {shared ? <div className="px-intel-chips"><span>◷ Early <small>Insider</small></span><span>⌘ Thermal key <small>Broker</small></span><span>⇢ Boat <small>Broker</small></span></div> : <div className="px-other-crew">Insider knows when. Broker knows the equipment and escape.</div>}
    {shared ? primary("Build a plan", () => advance(3)) : primary("Share with crew", () => { setShared(true); setNotice("Your route is public. Practice teammates shared their clues."); })}
    <span className="px-fine">Teammates are scripted in this practice, not live LLMs.</span>
  </section>;
  else if (step === 3) play = <section className="px-builder">
    <div className="px-plan-tabs" role="tablist" aria-label="Plan parts">{choices.map((choice, i) => <button key={choice.label} role="tab" aria-selected={part === i} onClick={() => setPart(i)}><span>{i + 1}</span>{choice.label}{visited.includes(i) ? " ✓" : ""}</button>)}</div>
    <span className="px-overline">PART {part + 1} OF 4</span><h2>{choices[part]!.label}</h2>
    <p className="px-evidence">{choices[part]!.hint}</p>
    <div className="px-options px-illustrated-options">{choices[part]!.options.map((option, i) => <button className={plan[part] === i && visited.includes(part) ? "selected" : ""} key={option} aria-pressed={plan[part] === i && visited.includes(part)} onClick={() => { setPlan(plan.map((value, j) => j === part ? i : value)); setVisited([...new Set([...visited, part])]); }}><ChoiceArt part={part} option={i} /><strong>{option}</strong><span className="px-card-pick">{plan[part] === i && visited.includes(part) ? "In your plan ✓" : "Add to plan"}</span></button>)}</div>
    <div className="px-plan-summary">{choices.map((choice, i) => <span key={choice.label}><small>{choice.label}</small>{visited.includes(i) ? choice.options[plan[i]!] : "Not chosen"}</span>)}</div>
    {part < 3 ? primary("Next: " + choices[part + 1]!.label.toLowerCase(), () => setPart(part + 1), !visited.includes(part)) : primary("Propose this plan", () => { advance(4); setNotice("Practice: plan proposed, planning window ended. Insider will choose this plan too."); }, visited.length !== 4)}
  </section>;
  else if (step === 4) play = <section className="px-commit">
    <span className="px-overline">YOUR PLAN / SEALED CHOICE</span><h2>The crew's way through.</h2>
    <div className="px-route-strip px-illustrated-route">{choices.map((choice, i) => <div key={choice.label}><ChoiceArt part={i} option={plan[i]!} /><small>{choice.label}</small><strong>{choice.options[plan[i]!]}</strong></div>)}</div>
    <label className="px-contribute"><input type="checkbox" checked={resource} onChange={e => setResource(e.target.checked)} /><div><strong>Contribute my resource</strong><p>The selected plan earns one resource point if at least one of its supporters contributes.</p></div></label>
    <p className="px-warning">Your choice is final for this round. In this practice, Insider votes for your plan; no other player contributes a resource.</p>
    {primary("Seal my choice", () => { advance(5); setNotice("Practice resolved with two votes for your plan. No live game was changed."); })}
  </section>;
  else play = <section className="px-result">
    <div className="px-result-title"><div><span className="px-overline">PRACTICE / CREW RESULT</span><h2>{score === 5 ? "A clean getaway." : score >= 3 ? "A partial failure." : "The plan fell apart."}</h2></div><strong>{score}<small>/ 5</small></strong></div>
    <p>Two crew members chose the same plan. {score === 5 ? "Every check passed." : "These checks explain the points this plan earned."}</p>
    <div className="px-checks">{choices.map((choice, i) => <div key={choice.label}><span className={checks[i] ? "pass" : "fail"}>{checks[i] ? "✓" : "×"}</span><span>{choice.label}<small>{choice.options[plan[i]!]}{checks[i] ? " · matches intel" : " · correct: " + choice.options[choice.correct]}</small></span><b>{checks[i] ? "+1" : "+0"}</b></div>)}<div><span className={resource ? "pass" : "fail"}>{resource ? "✓" : "×"}</span><span>Resource contribution<small>{resource ? "You contributed to the selected plan" : "No supporter contributed"}</small></span><b>{resource ? "+1" : "+0"}</b></div></div>
    {primary("Play the practice again", reset)}
    <span className="px-fine">Practice debrief. No live score was recorded.</span>
  </section>;

  const title = <div className="px-objective"><span className="px-overline">{phase.toUpperCase()} / {step === 0 ? "LEARN BEFORE YOU JOIN" : "YOUR NEXT MOVE"}</span><h1 ref={heading} tabIndex={-1}>{objectives[step]![0]}</h1><p>{objectives[step]![1]}</p></div>;
  const exitPractice = () => onExitPractice ? onExitPractice() : history.back();
  const hud = <header className="px-hud">{practiceMode && <button className="px-back" onClick={exitPractice}>← Back to catalog</button>}<button className="px-wordmark" onClick={reset}>AGENT<span>HEIST</span></button><nav aria-label="Game tools"><button onClick={openIntel}>My intel{inspected ? " · 1" : ""}</button><button onClick={() => setDrawer("crew")}>Crew · 3</button><button onClick={() => setDrawer("rules")}>How to play</button></nav><span className="px-clock">∞ <small>UNTIMED PRACTICE</small></span></header>;
  const scene = <div className="px-scene" style={{ backgroundImage: `linear-gradient(180deg,rgba(6,12,18,.03),rgba(6,12,18,.97)),url(${rooftop})` }}><span className="px-scene-caption">THE JOB / FIND A WAY IN. AGREE ON A WAY OUT.</span><div className="px-scene-bottom"><span className="px-role-badge">⌖</span><div><span className="px-overline">YOU ARE THE NAVIGATOR</span><h2>The crew needs your intel.</h2><p>No one starts with the whole picture.</p></div></div></div>;
  const phaseTrack = <ol className="px-track" aria-label="Practice progress">{chapters.map((chapter, i) => <li key={chapter} className={i === step ? "active" : i < step ? "done" : ""}><span>{i < step ? "✓" : String(i + 1).padStart(2, "0")}</span><b>{chapter}</b></li>)}</ol>;

  return <main className={`px-root variant-${variant} step-${step}`}>
    <div className="px-prototype-note">{practiceMode ? "GUIDED PRACTICE · SCRIPTED CREW · NO LIVE AI CALLS" : "INTERACTION STUDY · SIMULATED PRACTICE · NOT A LIVE ROOM"}</div>
    {hud}
    {variant === "A" ? <VariantA title={title} scene={scene} play={play} coach={coach} track={phaseTrack} feedback={feedback} /> : variant === "B" ? <VariantB title={title} play={play} coach={coach} track={phaseTrack} feedback={feedback} onIntel={openIntel} onCrew={() => setDrawer("crew")} /> : <VariantC title={title} play={play} coach={coach} track={phaseTrack} feedback={feedback} image={rooftop} />}
    {!practiceMode && <div className="px-switcher"><button aria-label="Previous design" onClick={() => cycle(-1)}>←</button><span>{variant} / {names[variant]}</span><button aria-label="Next design" onClick={() => cycle(1)}>→</button><button onClick={() => setDrawer("state")}>Study controls</button></div>}
    <dialog className="px-dialog" ref={dialog} onCancel={() => setDrawer(null)} onClose={() => setDrawer(null)}><div className="px-dialog-header"><b>{drawer === "rules" ? "The job, in plain English" : drawer === "intel" ? "Your intel" : drawer === "crew" ? "Your practice crew" : "Prototype state & controls"}</b><button autoFocus aria-label="Close panel" onClick={() => setDrawer(null)}>×</button></div>
      {drawer === "rules" ? <div className="px-rules"><p>Agent Heist is a timed cooperative planning game. You do not steer a character around a map.</p><ol><li><b>Discover:</b> open the intel assigned to your role.</li><li><b>Share and plan:</b> publish facts, trade privately, and propose or back a plan.</li><li><b>Commit:</b> choose one plan and whether to contribute a resource. Your choice is sealed.</li><li><b>Resolve:</b> at least two crew members must choose the same plan. No majority means failure and 0 points.</li></ol><h3>How the crew scores</h3><p>One point each for the correct route, timing, equipment and escape. One point if a supporter of the selected plan contributes a resource.</p><p><b>5: success. 3–4: partial failure. 0–2: failure.</b> There is no individual score or bonus for clicking quickly.</p><p>In the current rules, published intel must be true. You can withhold intel; you cannot publish a false clue.</p><p>Live rounds share a clock. Help, switching tabs, and leaving the page do not pause it. This local practice is untimed.</p><button className="px-primary" onClick={() => { setTour(true); setDrawer(null); }}>Show guidance</button></div> : drawer === "intel" ? <div className="px-rules"><span className="px-overline">{shared ? "SHARED" : "PRIVATE"}</span><h2>{inspected ? "Service entrance" : "Your dossier is still sealed"}</h2><p>{inspected ? "Your route clue. " + (shared ? "The whole crew can now see this." : "Only you can see this until you share it.") : "Open your route dossier in Discover. You do not need to know a clue identifier."}</p>{shared && <p>Shared by the practice crew: Early · Thermal key · Boat.</p>}</div> : drawer === "crew" ? <div className="px-rules"><p><b>Navigator · You</b><br />Find the route.</p><p><b>Insider · Scripted practice teammate</b><br />Find the entry time.</p><p><b>Broker · Scripted practice teammate</b><br />Find equipment and extraction.</p><p>Humans and agents use the same game rules. These two teammates are scripted locally; no model is running.</p></div> : <div className="px-rules"><p>Compare the same state in three different layouts. Arrow keys also switch designs. Nothing persists after reload.</p><div className="px-stage-picker">{chapters.map((chapter, i) => <button key={chapter} onClick={() => { setStep(i as Step); setInspected(i > 1); setShared(i > 2); setVisited(i > 3 ? [0,1,2,3] : []); setDrawer(null); }}>{chapter}</button>)}</div><button className="px-text-button" onClick={() => { reset(); setDrawer(null); }}>Reset practice</button><pre>{JSON.stringify({ variant, phase, step: chapters[step], inspected, shared, plan: choices.map((choice, i) => ({ part: choice.label, choice: visited.includes(i) || step >= 4 ? choice.options[plan[i]!] : null })), resource, sampleMajority: step === 5 ? 2 : null, sampleScore: step === 5 ? score : null, network: "none", simulated: true }, null, 2)}</pre></div>}
    </dialog>
  </main>;
}

function VariantA({ title, scene, play, coach, track, feedback }: { title: ReactNode; scene: ReactNode; play: ReactNode; coach: ReactNode; track: ReactNode; feedback: ReactNode }) {
  return <><div className="px-focus"><div className="px-focus-story">{scene}{track}</div><div className="px-focus-action">{title}{play}{coach}{feedback}</div></div></>;
}
function VariantB({ title, play, coach, track, feedback, onIntel, onCrew }: { title: ReactNode; play: ReactNode; coach: ReactNode; track: ReactNode; feedback: ReactNode; onIntel: () => void; onCrew: () => void }) {
  return <div className="px-tabletop">{title}<div className="px-table-layout"><aside className="px-table-crew"><span className="px-overline">AROUND THE TABLE</span><button onClick={onCrew}><b>◷</b>Insider<small>Timing</small></button><button onClick={onCrew}><b>⇄</b>Broker<small>Equipment + escape</small></button><button className="your-hand" onClick={onIntel}><b>⌖</b>You / Navigator<small>Open your hand ↗</small></button></aside><div className="px-table-play">{play}{feedback}</div></div><div className="px-table-bottom">{track}{coach}</div></div>;
}
function VariantC({ title, play, coach, track, feedback, image }: { title: ReactNode; play: ReactNode; coach: ReactNode; track: ReactNode; feedback: ReactNode; image: string }) {
  return <div className="px-story" style={{ backgroundImage: `linear-gradient(90deg,rgba(7,12,18,.94),rgba(7,12,18,.28)),url(${image})` }}><div className="px-story-narration"><span className="px-overline">AN INTERACTIVE CREW STORY</span>{title}{track}</div><div className="px-story-decision">{play}{coach}{feedback}</div></div>;
}
