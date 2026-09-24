# Second-post map design review — September 19, 2026

The selected renderer, public sample data, and exported pair are preserved in
[`tools/map-preview/`](../tools/map-preview/README.md). Run `npm ci` and `npm start`
in that directory, then open <http://127.0.0.1:4318/>. The gallery shows the
selected pair and concise reply. Production ingestion and publishing code have
not changed. Earlier option experiments remain in the ignored local
`state/map-options/` directory.

Selected pair: **N5 + C Transit atlas**. The revised aerial moves from zoom 16 to
16.58 (about 50% closer), keeps a centered red pin, and adds named restaurants,
cafés, and shops from the vector tiles. Collision handling prioritizes nearby
places, gives shops a small boost, and preserves station names. Fifteen places
are visible in the final sample. Both images are square so their paired Bluesky
thumbnails retain the complete cards.

The cards omit style names, preview labels, the radius ring and caption, and the
requested explanatory caveats. The address appears once. The latest revision
removes the 1/2/4 legend and both textual callouts for the highlighted application;
its red circle remains. The ward map expands into the freed space. Its left-aligned
caption reads “Application submitted April 1 2026 - September 19, 2026”.
Both cards right-align their credits, including “Data: City of Chicago Data Portal”.

## Scope and data

The user selected the citywide expansion cohort: applications submitted on or
after **April 1, 2026**, currently `Pre-Certified` or `Pre-Certified: admin adjust`.
This uses the existing City dataset `j4h8-ug9m`; no predecessor data is combined.

The complete local snapshot contains 502 applications, ingested September 19 at
12:22:59 p.m. America/Chicago. It yields 471 qualifying applications requesting
496 ADUs. Ward 43 contains 13 applications requesting 16 ADUs, tied for 11th of
all 50 wards with Wards 25 and 47. Its share is 16 / 496 = 3.2%, rounded to one
decimal place. Rank is 1 plus the number of wards with a strictly higher ADU sum.
Missing wards have zero; application IDs, not addresses, define identity.

One Ward 43 application requests four ADUs; the other twelve request one. The
2023 and 2025 N Racine records are separate neighboring applications. Their ward
markers use short leader lines to prevent overlap without moving the geographic
anchor. Numbers, size, and sequential shading encode quantities redundantly.

The City data API returned HTTP 503 during review. The saved ingestion snapshot
excludes coordinates. **These prototypes therefore use US Census address-range
geocoding, not source-record coordinates or surveyed rooftop locations.** All 13
addresses returned a single match, and all 13 points lie inside the official ward
polygon. Before production, inspect the source schema and use its coordinates if
available; retain provenance and explicitly identify any geocoding fallback.

The later live implementation uses the City's own latitude/longitude fields from
a complete new ingestion, not the prototype Census matches. It validates the
focus point against the Cook County ward boundary and reports coverage when
other City points cannot be mapped to that ward. The dated prototype data above
remains only a design reference.

## Map stack and rendering

- [MapLibre GL JS](https://maplibre.org/maplibre-gl-js/docs/examples/display-buildings-in-3d/)
  with [OpenFreeMap](https://openfreemap.org/) provides the recommended free route.
  OpenFreeMap requires no API key or per-request payment and has no SLA.
- The preview uses MapLibre 5.20.0, customized OpenFreeMap vector layers,
  and the repository’s Big Shoulders Bold and Roboto fonts.
- Building footprints and heights are OSM-derived. Extrusions use `render_height`
  and `render_min_height`; absent heights fall back to 8 m. Buildings depict
  existing context, not the proposed ADU. Attribution remains on the cards;
  source and coordinate provenance are in the gallery's collapsed source notes.
- The [City Data Terms of Use](https://www.chicago.gov/city/en/narr/foia/data_disclaimer.html)
  were checked on the official website in Browser on September 19, 2026. They
  require a disclaimer at the site where derived applications are accessed or
  downloaded; they do not prescribe a credit on each image. A City source credit
  is included in the images, and the gallery's source notes summarize the
  disclaimer and link the official terms. Production publishing should retain
  access to the required site-level statement.
- The neighborhood map is centered on application 954542 at 2057 N Sheffield Ave.
  The selected aerial uses zoom 16.58, pitch 60°, and bearing −42°. The earlier
  quarter-mile ring is removed. Perspective includes more distant context on
  the far side of the tilted view.
- Ward geometry comes from [Cook County’s Chicago Ward service, layer 28](https://gis.cookcountyil.gov/hosting/rest/services/cookviewer_political_boundaries/MapServer/28),
  queried for Ward 43. Its geographic bounding box is fit to the viewport with
  padding, and a separate rectangular frame sits within the image.
- Streets, rail infrastructure and stations come from OpenFreeMap/OpenMapTiles/OSM.
  Detailed rail geometry is extracted from zoom-14 vector tiles so transit tracks
  survive at the smaller ward scale. These maps are geographic context, not
  scheduled service or frequency diagrams.
- [Protomaps](https://docs.protomaps.com/basemaps/layers) is a viable alternative
  for a self-hosted Chicago PMTiles extract. It includes building height fields;
  its documented transit layer is empty, so rail/station context needs separate
  handling. Storage and hosting may have costs even with open-source software.
- Selected exports are 2160 × 2160 JPEGs, individually under 2,000,000 bytes.
  The original option exports were 2160 × 2700.

## Proposed reply

> Ward 43 has 16 ADUs across 13 preapproved applications.
>
> Tied #11 of 50 wards by requested ADUs — 3.2% of the citywide total.
>
> Applications submitted since Apr 1, 2026. As of Sep 19.

The updated live test uses this concise text and includes coordinate provenance
in the image alt text. It attaches the neighborhood image first and ward image second to one
reply. Its `root` and `parent` strong references both point to the existing first
post. The separate test script freezes the payload and identity before sending,
then verifies the stored remote record; retries reconcile the same identity.
The selected pair is live at
<https://bsky.app/profile/chiadupreapprovals.bsky.social/post/3mvvwzostgf22>.
Its receipt is `state/map-options/renders/selected-live-receipt.json`; the frozen
payload is `state/map-options/selected-v2-live-frozen.json`. The first option test
remains in the feed for the user's review/deletion.

In the versioned preview, open `/render-selected.html` to regenerate the two
JPEGs and `selected-verification.json` under `tools/map-preview/renders/`.
Use `npm run refresh-map-context` only to refresh the rail and POI extracts from
the saved tile version. It does not refresh application facts or statistics.
The local `state/map-options/post-selected-v2.py` script reconciles this specific
live test identity on repeat runs; it is not part of the versioned preview.

## Implementation after selection

1. Add a versioned source/enrichment contract for coordinates and ward geometry.
   Validate bounds, coordinate provenance, ward membership, quantities and dates.
2. Derive the whole-ward cohort and all-50-ward ranks from one complete successful
   snapshot. Preserve its timestamp, definition, counts and map-point evidence.
   If a location is unavailable, omit its pin and report mapped/total coverage;
   never place it at a ward centroid. Unknown quantities must not become one.
3. Generate and freeze both map images, their alt text, statistics and source
   attribution. Map failure can defer the reply while preserving the successful
   first post. The render worker requires a browser/WebGL runtime; budget that
   separately from the small native Rust polling process and cache outputs.
4. Give the reply its own stable delivery identity/state. Send only after the
   parent has a verified URI/CID. Use the original root and the first post as
   parent. Reconcile ambiguous writes and retry the existing reply identity;
   never re-send the parent to recover a reply failure.
5. Test thread linkage, multiple image order, snapshot/rank ties, zero and unknown
   quantities, missing coordinates, adjacent and repeated addresses, irregular
   ward bounds, map failures and restart reconciliation before enabling automatic
   second posts.

Review validation: all eight renders were inspected; MapLibre reported no errors;
all 13 example points were inside Ward 43; the full boundary had padding; nearby
stations and detailed rail lines were visible; JPEG dimensions and byte limits
were checked. The gallery’s selection/paired-view controls were tested in Browser.
The selected revision was additionally inspected after export: both square JPEGs
are below the upload limit, all 13 application markers remain present, adjacent
markers have leader lines, and the complete ward boundary retains padding.
