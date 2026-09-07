import { createProductionPlatformBff } from "@worldstream/platform";

const platform = createProductionPlatformBff();

/** Vercel owns HTTPS control-plane requests. Fly remains the only WebSocket host. */
export default {
  fetch(request) {
    return platform.fetch(request);
  },
};
