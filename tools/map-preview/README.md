# Second-post map preview

The selected N5 neighborhood map and C Transit atlas for application 954542 at
2057 N Sheffield Ave. This is a frozen design example dated September 19, 2026.
The live `render-live.mjs` worker follows this design using a current, source-backed
snapshot; run it through `adu-bot scorecards preview` to inspect the resulting pair.

```sh
cd tools/map-preview
npm ci
npm start
```

Open <http://127.0.0.1:4318/> to review the saved pair. Open
<http://127.0.0.1:4318/render-selected.html> to regenerate both 2160 × 2160 JPEGs.
The renderer needs a browser with WebGL and network access to OpenFreeMap for
vector tiles, glyphs, and sprites. It writes images and verification metadata
into `renders/` through the local server. Set `MAP_PREVIEW_PORT` to use another
port. Rendering and serving do not authenticate or publish to Bluesky.

The renderer uses bundled project fonts and the pinned MapLibre npm dependency.
N5 uses white building extrusions, zoom 16.58, pitch 60°, bearing −42°, a centered
pin, and nearby restaurant, café, and shop labels. The ward image fits the full
boundary with padding and uses numbered, shaded markers for requested ADUs.
The selected design omits the legend and callout text, uses a left-aligned date
caption, and right-aligns source credits.

Permit replies use this same live renderer with a permit snapshot. Their ward
markers represent preapproved sites with confirmed issued building permits;
the marker number counts permits at that site, and a red ring identifies the
current permit's site. This keeps permit counts separate from requested ADUs.

## Public sample data

- `points.json`: 13 public project locations and requested ADU quantities from
  the [City of Chicago ADU dataset](https://data.cityofchicago.org/d/j4h8-ug9m).
  Coordinates are Census address-range matches, not rooftop coordinates.
- `summary.json`: ward and city totals for submissions since April 1, 2026,
  currently Pre-Certified or Pre-Certified: admin adjust in the saved snapshot.
  Ward 43 has 16 ADUs across 13 applications, tied for 11th of 50 wards.
- `ward43.json`: official boundary from
  [Cook County's Chicago wards service](https://gis.cookcountyil.gov/hosting/rest/services/cookviewer_political_boundaries/MapServer/28).
- `base-style.json`, `tilejson.json`, `rail.json`, `poi.json`: OpenFreeMap style
  and geographic context derived from OpenMapTiles / OpenStreetMap. Map data is
  subject to the [OpenStreetMap ODbL](https://www.openstreetmap.org/copyright);
  the [OpenMapTiles attribution](https://openmaptiles.org/docs/website/attribution/)
  is retained in the rendered images. The raster examples preserve these credits.

`npm run refresh-map-context` downloads the detailed tiles referenced by the
saved TileJSON and rebuilds rail and POI extracts. Tile downloads are ignored by
Git. This does not refresh application facts, dates, or ward statistics. The
sample contains only the project fields used for the map; local snapshots,
credentials, sessions, and live-post payloads remain outside this directory.

## Live scorecard rendering

`adu-bot scorecards preview APPLICATION_ID --output-dir DIR` creates a current
snapshot from the ingested City rows and official Cook County ward boundary,
then launches `render-live.mjs` with headless Chrome. The renderer reads only that
frozen input. It exports two 2160 × 2160 JPEGs and a verification file, and the
CLI writes the same maps, alt text, post text, and source snapshot into `DIR`.
It requires a successful post-migration ingestion, Node 20+, Chrome with
software WebGL, and outbound OpenFreeMap access. The bot's scorecard worker uses
the same renderer before uploading a reply. The prototype `points.json` and
`summary.json` remain dated samples; they do not feed live replies.

See [the design review](../../docs/map-design-review.md) for data provenance,
City attribution terms, validation, and the remaining production integration.
