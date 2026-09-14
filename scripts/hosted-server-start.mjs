const SERVER_START_RETRY_DELAY_MS = 1_000;
const SERVER_START_MAX_ATTEMPTS = 30;

/** Retry only the Controller's explicit recoverable Runtime-start checkpoints. */
export async function retryHostedServerStart(
  start,
  pause = delay,
  maximumAttempts = SERVER_START_MAX_ATTEMPTS,
) {
  let result = await start();
  for (let attempt = 1;
    attempt < maximumAttempts && isTransientHostedServerStartFailure(result);
    attempt += 1) {
    await pause(SERVER_START_RETRY_DELAY_MS);
    result = await start();
  }
  return result;
}

function isTransientHostedServerStartFailure(result) {
  if ((result?.code !== 3 && result?.code !== 4) || typeof result.stdout !== "string") return false;
  try {
    const report = JSON.parse(result.stdout);
    if (report?.command !== "server start") return false;
    return (result.code === 3 && report.code === "controller_unavailable") ||
      (result.code === 4 && report.code === "lifecycle_incomplete" && report.stage === "runtime_restart");
  } catch {
    return false;
  }
}

function delay(milliseconds) {
  return new Promise((resolvePromise) => setTimeout(resolvePromise, milliseconds));
}
