import { createProductionPlatformBff } from "@worldstream/platform";
import { describeDeploymentRequestBoundary } from "../requestBoundaryDiagnostic.mjs";

const platform = createProductionPlatformBff();

/** Vercel owns HTTPS control-plane requests. Fly remains the only WebSocket host. */
export default {
  fetch(request) {
    const diagnostic = describeDeploymentRequestBoundary(request);
    if (diagnostic !== null) console.info(JSON.stringify(diagnostic));
    return platform.fetch(request);
  },
};
