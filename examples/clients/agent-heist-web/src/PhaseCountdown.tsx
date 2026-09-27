import { useEffect, useState } from "react";

/** Presentation only: the authorized Projection, not this clock, advances the phase. */
export function PhaseCountdown({ deadline, connected }: {
  readonly deadline: string | null;
  readonly connected: boolean;
}) {
  const deadlineMs = deadline === null ? NaN : Date.parse(deadline);
  const [now, setNow] = useState(Date.now);
  const valid = Number.isFinite(deadlineMs);

  useEffect(() => {
    if (!connected || !valid) return;
    const refresh = () => setNow(Date.now());
    // Recalculate from the absolute deadline after a background tab resumes.
    const interval = window.setInterval(refresh, 1_000);
    document.addEventListener("visibilitychange", refresh);
    return () => {
      window.clearInterval(interval);
      document.removeEventListener("visibilitychange", refresh);
    };
  }, [connected, valid]);

  if (deadline === null) return <strong>No active timer</strong>;
  if (!valid) return <strong>Timer unavailable</strong>;
  if (!connected) return <strong>Reconnect to update timer</strong>;

  const seconds = Math.max(0, Math.ceil((deadlineMs - now) / 1_000));
  const clock = `${String(Math.floor(seconds / 60)).padStart(2, "0")}:${String(seconds % 60).padStart(2, "0")}`;
  return <>
    <strong className={`phase-countdown${seconds <= 10 ? " phase-countdown-urgent" : ""}`}
      role="timer" aria-label="Time remaining in current phase" aria-live="off"
      title={`Phase deadline: ${new Date(deadlineMs).toLocaleString()}`}>
      {clock}
    </strong>
    {seconds === 0 ? <span className="phase-countdown-note" role="status">Waiting for the next phase…</span> : null}
  </>;
}
