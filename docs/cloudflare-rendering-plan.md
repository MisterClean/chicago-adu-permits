# Cloudflare map rendering implementation plan

Move the existing map renderer to Cloudflare Browser Run through a small Node controller. Keep SQLite, source snapshots, copy, alt text, uploads, and posting identities in the Rust application. Start with the tested rendering optimizations, then cache the fixed ward basemaps so native Rust can add changing markers without a browser.

Implemented on `codex/cloudflare-map-renderer`, following local tests on October 5, 2026. Production configuration, code, database, and schedules remain unchanged. The backend defaults to local Chrome until explicitly configured.

The shared Rust adapter now serves both reply types. It includes bounded controller execution and diagnostics, exact pair caches, frozen permit map evidence, account budget reservations, typed quota deferrals and credential holds, clean ward backgrounds, and native marker composition. `render-maps`, `render-ward`, and resumable `warm-maps` run without opening SQLite or authenticating a social publisher. CI packages the controller with its dependencies into the updater's existing seven-asset layout.

The integrated release upgrades MapLibre from 5.20.0 to patched 6.12.0. Its bundled module worker needs a concrete document origin on Cloudflare. The controller fulfills a reserved `.invalid` document locally through CDP, then injects all assets; no public render page is deployed. Browser page errors and MapLibre's per-image errors fail the render. Console network notices are retained as diagnostics on failure rather than treating a recovered connection reset as a failed image.

See [operating the backend](operations.md#cloudflare-map-renderer) for configuration, commands, and recovery.

## Integrated acceptance results

The final seven-asset release bundle rendered the Ward 26 preapproval pair in 19.15 seconds with empty MapLibre error arrays, valid 2160-square JPEGs, and a clean 2160×1600 background. An earlier integrated success took 56.93 seconds; these isolated samples show substantial startup/network variation and do not establish a new average. The patched MapLibre output is visually inspected rather than asserted byte identical to 5.20.0.

An exact repeat took 0.063 seconds and returned byte-identical images with a deliberately unavailable Cloudflare token file. A synthetic permit fixture using two public Ward 26 locations and a different focus reused the same background: its verification contained only `n5`, proving that no ward map was rendered remotely. That pair took 21.87 seconds. The focus and tiles differ from the preapproval sample, so the pair times are not a controlled cache speed comparison. Its ward-only native render took 0.168 seconds with the token unavailable; both permit images were visually checked. These diagnostics created no SQLite database and posted nothing.

A sampled sum of Rust/Node RSS during one remote attempt peaked near 135 MiB on macOS. It is not a Linux cgroup measurement. A standalone ward composition measured roughly 57 MiB peak RSS. Linux memory/task acceptance under production's 192 MiB/24-task limits remains required before deployment.

Automated checks cover actual MapLibre projection references, moved focus rings and fresh permit counts on a shared background, corrupted/changed frozen evidence, incomplete or mismatched exports, oversized artifacts, conservative crash reservations and UTC rollover, numeric/date `Retry-After`, credential isolation from Bluesky, and owned-session cleanup when CDP closure fails or hangs. Existing root/reply recovery, ambiguous writes, and database migration checks remain in the suite. Live tests confirmed no active browser sessions after cleanup. Raw test evidence remains ignored under `state/cloudflare-implementation`.

## Original optimization benchmark (MapLibre 5.20.0)

Two instrumented baseline runs and three optimized runs used the same current Ward 26 preapproval snapshot: 37 applications and 38 requested ADUs. Each pair started a fresh remote browser; its HTTP cache was cleared before rendering. The upstream CDN cache was uncontrolled.

| Measurement | Existing renderer | Optimized renderer |
| --- | --- | --- |
| Pair wall time including connection and closure | 36.3 and 44.1 seconds | 15.5, 23.4, and 30.0 seconds |
| Mean pair wall time | 40.2 seconds | 23.0 seconds |
| Mean measured network traffic | 1.77 MB | 1.37 MB |
| Ward vector tile requests | 7 | 6 |
| Output dimensions | 2160 by 2160 | 2160 by 2160 |
| Neighborhood and ward JPEG sizes | 1.26 MB and 0.81 MB | 1.26 MB and 0.81 MB |

The combined optimization reduced mean wall time by 42.8% and network traffic by 22.7%. Vector tile traffic alone fell by 12.2%; the remaining savings include removing unused sprites. Every optimized JPEG was byte identical to the corresponding baseline JPEG, and all marker projections and final cameras matched. Both map styles were visually inspected and reported no map errors.

The tested changes were:

1. Set the ward bounds and existing padding in the MapLibre constructor. The current renderer starts the ward map at neighborhood zoom, waits for tiles, and only then calls `fitBounds`.
2. Wait for `style.load` before adding the ward overlays and neighborhood place labels, then retain the final `idle` wait. This avoids waiting for an initial map load before creating the final scene.
3. Remove the sprite and unused `ne2_shaded` raster source. None of the surviving layers uses an icon, and all geographic layers use the vector source.

The third optimized run verified the reusable saved harness and confirms that startup and network variation remain material. The camera change alone did not improve the first timing comparison: 34.0 versus 35.5 seconds. The useful result is for the three changes together; this experiment does not isolate each change's contribution.

This is a small historical benchmark for one ward and one focus location; byte identity applies to the original 5.20.0 comparison, not the subsequent dependency upgrade. It measures wall time and CDP `encodedDataLength`, including response overhead, rather than authoritative Cloudflare billing duration. The preapproval branch was tested; permit fixtures remain part of implementation acceptance.

Evidence is preserved locally in `state/cloudflare-test-20261005/optimization/summary.json`, individual `report.json` files, and the exported JPEGs. Those files are ignored by Git. The reusable harness is in [tools/cloudflare-benchmark](../tools/cloudflare-benchmark/README.md).

## Filtering before downloading

The application already filters its ADU points to the relevant ward, and `scorecard::fetch_boundary` asks Cook County for `WARD=<number>` with only the ward attribute and geometry. Keep those filters.

The basemap source named `planet` returns tile metadata; it does not download the planet. MapLibre subsequently requests geographic vector tiles for the camera. In the instrumented baseline the neighborhood needed four tiles at zoom 14. The ward first requested one zoom 14 tile, then six zoom 13 tiles. Initializing the final ward camera removed that extra zoom 14 request before download.

Use the final camera as the first spatial filter. If a custom regional tile source is added later, its `bounds` property can prevent requests outside a padded coverage rectangle. Include the full visible context around the ward and the footprint of pitched neighborhood views; restricting the source to the exact ward polygon would remove streets outside the border. MapLibre documents both [initial camera bounds](https://maplibre.org/maplibre-gl-js/docs/API/type-aliases/MapOptions/#bounds) and [source bounds](https://maplibre.org/maplibre-style-spec/sources/#bounds).

Individual vector tile URLs contain a complete tile. A MapLibre layer filter changes what is drawn after download. It cannot make OpenFreeMap return only selected buildings, roads, or POIs inside that tile. Filtering or simplifying tiles in our own service would require fetching the upstream tile first, and would primarily help subsequent cached requests. Do not build that service for the first release.

For an optional standalone City data query, the numeric `latitude` and `longitude` fields allow a bounding rectangle predicate, and `ward = 26` can limit rows. Prefer the existing validated local snapshot for render jobs. The bot's ingestion must still scan the complete source: citywide ranks, historical changes, and delivery eligibility depend on that complete scan. A partial location query must never replace the production source snapshot.

## Implemented design

### Share the renderer across reply types

Add a small shared renderer module used by both `src/maps.rs` and `src/publish/scorecards.rs`. Both currently launch `render-live.mjs` separately. Keep their existing snapshot construction, geographic validation, and alt text responsibilities; route only the image generation through the new adapter.

Add a `[maps]` configuration section independent of `scorecards.enabled`, since issued permit maps use the renderer even when preapproval scorecards are disabled. Options are backend, account ID, token file, renderer directory, Node path, attempt timeout, and daily budget. Keep local Chrome as an explicit development backend. A Cloudflare failure should schedule a reply retry rather than silently launch Chrome on the constrained production host.

### Add the remote controller

Create a production `render-cloudflare.mjs` with the same snapshot input and output directory contract as `render-live.mjs`. Use pinned `puppeteer-core` to connect to the [Browser Run CDP endpoint](https://developers.cloudflare.com/browser-run/cdp/puppeteer/). Chromium and WebGL run on Cloudflare; Node on our host handles transport and bounded output files.

Pass the allowlisted map snapshot, existing bundled MapLibre script, base style, and fonts to the browser. Refactor the renderer to accept injected inputs and an export callback, keeping the existing local preview transport available. Integrate the three tested optimizations into this shared code instead of relying on the benchmark's string substitutions.

Use one browser per pair and render the images sequentially. Always close the browser in `finally`, including page errors, export failures, and signals. Acquire the session through the documented HTTP API and retain its session ID before connecting over CDP, so interrupted jobs can close only their own abandoned session. The benchmark uses direct WebSocket acquisition; the integrated controller uses explicit HTTP acquisition and tests owned-session cleanup, including failed and hanging CDP closure. Give the Node attempt a 90 second deadline and the Rust parent a slightly longer cleanup deadline, both bounded by the remaining application run time. These defaults still require larger-ward and Linux resource acceptance before cutover.

Write temporary JPEGs and a manifest, then atomically promote a complete pair. Do not send image bytes as unbounded stdout. The manifest should include the complete input digest, mode, ward, focus ID, source run, renderer version, image hashes, dimensions, bytes, and map errors.

### Preserve verification and delivery recovery

Rust must verify the manifest against its frozen input and check both JPEG formats, dimensions, and the 2 MB limit before uploading either image. Keep the requested ADU and issued permit meanings distinct. Verify both branches against representative long addresses and irregular wards.

Use an exact pair cache under the existing state directory, keyed by the complete render payload and versioned assets. Hash the complete payload, boundary, style, fonts, packaged controller/MapLibre assets, native composition source and locked dependencies. The weekly epoch bounds freshness of the unpinned upstream tile source. Commit both images and their manifest as one complete cache entry. Reuse that pair for render or upload retries; do not reuse stale counts, dates, or another focus location.

Preserve the existing root and reply identities, frozen snapshots, remote URI and CID receipts, and ambiguous write reconciliation. A render failure retries the existing reply and never resends a successful root. No database reset or schema migration is needed for the first adapter and file cache.

The current process owns `writer.lock` throughout remote preparation. Keep attempts bounded for the first release. Splitting rendering into a separate claimed-job worker would require its own durable claim and snapshot validation design; defer that change until lock contention is measured after integration.

### Credentials and resource limits

Use a dedicated account-scoped API token with Browser Rendering Edit permission, read from a protected file. Wrangler OAuth is suitable for this local benchmark; do not copy its personal OAuth configuration to production. Keep Cloudflare authentication in the controller's connection header, outside the browser page and snapshot. Bluesky, Google, and database credentials remain on the host.

Measured optimized controller peaks were about 99 to 104 MiB RSS on macOS. That is not the combined Rust and Node footprint on Linux. Before cutover, measure the combined cgroup peak, CPU, task count, and cleanup under the production limits of 192 MiB memory and 24 tasks. Target at least 32 MiB of memory headroom. The remote browser eliminates Chrome's host memory, but a Node controller still has a measurable cost.

If combined memory fails that acceptance check, test a Rust HTTP Quick Actions adapter or a smaller CDP controller before changing production limits. Quick Actions would need a separate fidelity and timing test because its screenshot and export contract differs from the tested CDP path.

## Free quota and retries

The account is currently on Workers Free. [Browser Run allows 600 browser seconds per day, three concurrent browsers, and one new browser every 20 seconds](https://developers.cloudflare.com/browser-run/limits/). Its quota resets on the next UTC day, which is 7 p.m. Chicago time on this test date and shifts with daylight saving time.

At the optimized mean wall time, 600 seconds suggests about 26 pairs per day; at the slowest optimized sample, about 19. These are planning estimates before retries and shared account use, not measured billing capacity. Begin with a maximum of 16 new pairs per day and a conservative local time budget, then adjust from actual session history. Rendering 50 pairs daily still exceeds the allowance at the measured pace.

Persist attempt reservations and measured elapsed time in the state directory under the existing process lock. Reserve time before starting and charge interrupted attempts conservatively. This counter cannot see other applications on the account; Cloudflare's quota response remains authoritative.

Space browser acquisitions at least 20 seconds apart. Start sequentially. A reused browser for a bounded batch is a later option, but never keep it idle between scheduled runs. Idle sessions consume quota.

Honor `Retry-After` for acquisition limits. A daily quota error should defer the same reply until the next UTC reset. Invalid credentials should produce an actionable hold. Network and tile failures get bounded backoff. Preserve quota for retries; additional parallel browsers lower wall time but do not inherently reduce total browser seconds.

## Cache the 50 ward basemaps

The ward map has a fixed north-up camera and background for each ward. Render a clean background containing streets, labels, boundary, and outside fade once per version. Store a lossless image and its exact camera transform, logical viewport, pixel ratio, projection, and padding. Cache keys should include the ward boundary, style, renderer and basemap versions; invalidate on a boundary or cartographic change.

Use the existing Rust `tiny-skia`, `fontdue`, and `image` dependencies to add current dots, collision offsets, leader lines, the highlighted focus, headings, counts, dates, and attribution. Validate native point projection against MapLibre coordinates, including points near irregular borders. Store the clean background rather than an image that already contains yesterday's dots or totals.

Keep these backgrounds in the local state directory initially. R2 or a static asset service can be added if sharing the cache becomes useful; it is not required for the first release. Generate only missing backgrounds and spread the initial 50 across days to stay within quota.

The neighborhood map changes its center for every focus and uses pitch and bearing. It still needs on-demand Browser Run unless we change its cartographic design or cache that exact focus. Do not assume that 50 ward backgrounds replace all neighborhood views.

Native composition is implemented and tested against exported MapLibre coordinates at the focus and ward edges. A source-backed Ward 26 background produced a ward JPEG in 0.18 seconds locally, with about 57 MiB peak Rust RSS on macOS, without Node or Chrome. That is a standalone diagnostic measurement, not the combined Linux production cgroup footprint. Fresh focus markers and both preapproval/permit copy branches are covered by native image tests.

## Acceptance and rollout

1. Refactor the transport and add the Cloudflare backend with publication disabled. Keep the existing JPEG and snapshot validation, and add failure fixtures for partial exports, incorrect manifests, dropped sessions, quota errors, and timeouts.
2. Render preapproval and permit fixtures for dense and irregular wards, a boundary location, and long labels. Compare markers, camera, text, attribution, and image order. Establish timing and combined Linux memory headroom before cutover.
3. Exercise root success with reply failure, restart after rendering, cached pair reuse, upload retry, and ambiguous social writes on a disposable database copy. Verify IDs and receipts survive unchanged.
4. Prepare an immutable release and configuration change. Production deployment remains a separate action. On authorized cutover, switch only the map backend, preserve the database and schedule, and observe the next intended run. Do not enable preapproval scorecards as a side effect.
5. Retain the previous release for code rollback. Preserve newer receipts and prepared payloads during rollback; do not restore an older database merely to undo the renderer change.
6. Keep native ward composition configurable. Projection and freshness tests pass; Linux resource acceptance and observation of other ward shapes remain part of cutover.
