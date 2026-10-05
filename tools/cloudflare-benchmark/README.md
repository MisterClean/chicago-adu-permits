# Cloudflare map rendering benchmark

This developer tool compares copies of the historical renderer in `baseline-render.js` on Cloudflare Browser Run. It does not modify application code, production state, or publish posts. It uploads the supplied map snapshot and public rendering assets to Cloudflare, creates browser sessions, downloads JPEGs, and closes the sessions. Use a map snapshot containing only the public fields required by the renderer.

Use Node 22.12 or later, install the existing MapLibre preview dependencies, then install this tool's pinned controller dependency:

```sh
npm ci --prefix tools/map-preview
npm run build-vendor --prefix tools/map-preview
npm ci --prefix tools/cloudflare-benchmark
```

Authenticate Wrangler with account read, user read, and browser write scopes, or provide `CLOUDFLARE_API_TOKEN` privately in the environment. This tool obtains the Wrangler token without printing it. It does not require a Worker deployment.

```sh
CF_ACCOUNT_ID=YOUR_ACCOUNT_ID node tools/cloudflare-benchmark/benchmark.mjs \
  --snapshot state/your-map-snapshot.json \
  --out state/cloudflare-benchmark \
  --variants baseline,lean,lean,baseline
```

Each sample creates a fresh browser and clears its HTTP cache. Acquisition starts are spaced at least 20 seconds apart. The `lean` variant sets final ward bounds before fetching tiles, waits for `style.load` before adding overlays, and removes unused sprite and raster assets. The `final-camera` variant isolates the camera change. The final `idle` wait and JPEG quality remain unchanged.

Per-sample outputs include both JPEGs, verification with camera and marker positions, a network trace including MapLibre worker requests, controller RSS, and wall time through browser closure. A batch JSON contains the full reports. Network bytes are CDP `encodedDataLength`, not decoded geometry size or authoritative billing usage. The benchmark deliberately pauses new map workers briefly to attach network observation in both variants.

The source substitutions are guarded against the frozen historical renderer (main at the start of this implementation). Both variants use the currently installed MapLibre dependency; the October 5 initial measurements used version 5.20.0, while the integrated release uses patched 6.12.0. They are an experiment, not the proposed production adapter. Repeated tests consume the account's daily Browser Run allowance; default four-sample runs are intended for a bounded comparison.

See [the implementation plan](../../docs/cloudflare-rendering-plan.md) for measured results, constraints, and rollout steps.
