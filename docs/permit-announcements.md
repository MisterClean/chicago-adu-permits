# Building-permit announcements

The new series follows an issued City of Chicago building permit linked to a current preapproved ADU application. It uses one event identity per **preapproval application and permit number**. A two-ADU project can therefore produce two permit posts when separate permits cover separate work. Permit issuance does not mean an ADU has been built or occupied.

## Source and match contract

The permit adapter scans [Building Permits `ydr8-5enu`](https://data.cityofchicago.org/Buildings/Building-Permits/ydr8-5enu/about_data) for records issued on or after April 1, 2026 whose descriptions or conditions mention ADUs, accessory/additional dwelling units, coach houses, conversion units, dwelling units, or `D.U.`. It fetches only ID, permit status/type, application and issue dates, address, description/condition, reported cost, ward, and coordinates. It does not fetch contact names. A bounded keyset scan checks count and metadata revision before and after staging; incomplete scans do not change current permits or events.

Candidates require a preapproved application, the same street number and direction, and a normalized street name. Street suffixes are standardized, and a one-character street spelling difference is offered for review. Only active/complete renovation or new-construction permits qualify as building permits. An explicit `ADU ID <application>` in the permit text plus an agreeing address is the strongest link. A unique exact address with explicit ADU/coach-house/conversion scope can be confirmed automatically when the permit and preapproval wards agree and issue date follows preapproval. Generic dwelling-unit scope, fuzzy spelling, ambiguous applications, ward conflicts, missing/future dates, and reversed dates require review. A permit application can begin before preapproval; that alone does not disqualify the link.

The first complete permit scan silently baselines existing permits, including confirmed links. Later historical records with issue dates before the baseline are suppressed. Match reviews are append-only, while posted record keys and receipts remain stable across retries. Confirm a proposed link only after comparing both official records:

```sh
adu-bot --config config.toml permit-matches list
adu-bot --config config.toml permit-matches confirm 914381 101083804 --reason 'ADU ID and two-unit permit scope agree'
adu-bot --config config.toml permit-matches reject 914381 101083804 --reason 'Permit is for different work'
adu-bot --config config.toml queue inspect chicago:j4h8-ug9m:914381:building-permit:101083804:v1
```

## Post and maps

The root post gives the issue date, project address, requested ADU count, and, when present and credible, **reported project cost**. This cost covers the reported permit project, which may include other building work; it is not an ADU construction invoice. It links to the permit and preapproval records. Three calendar intervals appear when their source dates exist in chronological order: preapproval to permit application, permit application to issue, and preapproval to issue. An invalid or reversed interval is omitted.

The root card now leads with a green check and **ADU BUILDING PERMIT ISSUED**, while keeping the existing Chicago flag blue/red, Big Shoulders/Roboto, Street View context, and unofficial-feed treatment. Its reply uses the same MapLibre neighborhood and ward design as the preapproval scorecard: an oblique 3D neighborhood view and an outlined ward view, both with a red pin at the approximate permit point. The reply text computes requested ADUs, applications, rank, and city share from one current preapproval snapshot (submissions since April 1, 2026). Map context comes from OpenMapTiles/OpenStreetMap, the ward boundary comes from [Cook County GIS](https://gis.cookcountyil.gov/hosting/rest/services/cookviewer_political_boundaries/MapServer/28), and permit coordinates come from the City record. Credits and point limitations appear on the cards and in alt text. Maps are geographic context, not surveyed property boundaries.

The reply has an independent outbox identity and frozen payload. It is prepared only after the root has a verified URI/CID; an upload, map, or reply failure does not resend the successful root. Both map JPEGs are individually capped at Bluesky's 2 MB image limit.

## Rollout

Existing databases require the explicit schema-3 migration in the normal updater or `migrate` procedure; schema 2 is the merged scorecard migration. Keep publishing disabled during migration and the first permit baseline. Run `ingest-permits`, inspect `status --json` and `permit-matches list`, preview representative examples, then resume the usual `run` schedule. The service fetches the permit source when its six-hour ingest interval is due. `check --health` requires a fresh permit scan after activation. No historical permit posts are created automatically.

```sh
adu-bot --config config.toml preview-permit --application-id 914381 --permit-number 101083804 --image /tmp/agatite.jpg
adu-bot --config config.toml publish --dry-run
```

The image preview writes the root card, `.near.jpg` and `.ward.jpg` map images, and alt-text siblings. It does not authenticate or post. The root card uses the existing Google Street View key. Map rendering requires the bundled renderer, Node 20+, Chrome with software WebGL, and public Cook County and OpenFreeMap access. The permit reply is currently prepared by the main publishing worker, so its host needs enough memory for Chrome even if the separately scheduled scorecard worker is disabled. If rendering fails, the independent reply delivery retries and the root receipt remains intact.

## Local verification (September 24, 2026)

A disposable complete scan found 512 preapproval applications and 1,189 permit candidates; 20 permit/application pairs met the automatic evidence rule and nine required review. All were baseline-suppressed or proposed, so this test created no public posts. The Agatite example rendered the updated portrait permit card and both MapLibre maps within Bluesky's image limit. Live Bluesky delivery and a real new-permit transition remain unobserved.
