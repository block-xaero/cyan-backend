#!/bin/bash
# ============================================================================
# seed_demo.sh — Seed realistic demo data for Cyan NAB 2026
#
# Creates:
#   - Clean group/workspace/board hierarchy for Planetcast Media
#   - Pipeline board with seeded outputs for all 6 steps
#   - Timecoded notes on video from QC/compliance/SSAI
#   - Chat messages showing collaboration
#   - Email triage board for agentic workflow demo
#
# Usage:
#   ./seed_demo.sh
#
# Prerequisites:
#   - Cyan app must have been run at least once (DB exists)
#   - Will REPLACE existing demo data
# ============================================================================

set -e

DB="$HOME/Library/Containers/6E1945B9-5A6A-49BF-9826-E1F4A7D5AF89/Data/Documents/cyan.db"

if [ ! -f "$DB" ]; then
    echo "❌ Database not found. Run the Cyan app first."
    exit 1
fi

echo "🌱 Seeding Cyan demo data for NAB 2026..."
echo "   DB: $DB"

# ============================================================================
# Helper: generate blake3-style hex IDs
# ============================================================================
gen_id() {
    python3 -c "import hashlib,time,random; print(hashlib.sha256(f'$1-{time.time()}-{random.random()}'.encode()).hexdigest())"
}

NOW=$(date +%s)
HOUR_AGO=$((NOW - 3600))
TWO_HOURS_AGO=$((NOW - 7200))
DAY_AGO=$((NOW - 86400))

# ============================================================================
# IDs — use the EXISTING pipeline board ID so we don't lose pipeline configs
# ============================================================================
PIPELINE_BOARD="c69e29968a2d900158403422596a263354168c5da8a7f0d0e67615c391e47656"

# Check if pipeline board exists
EXISTING=$(sqlite3 "$DB" "SELECT COUNT(*) FROM notebook_cells WHERE board_id='$PIPELINE_BOARD' AND metadata_json LIKE '%pipeline%';")
echo "   Found $EXISTING pipeline cells in existing board"

# ============================================================================
# Step 1: Seed pipeline outputs (keep existing cells, update outputs)
# ============================================================================
echo ""
echo "📺 Seeding pipeline outputs..."

sqlite3 "$DB" << 'SQLEOF'

-- ── QC Step Output ──────────────────────────────────────────
UPDATE notebook_cells SET output = '## QC Analysis — Big Buck Bunny

### Step 1: Video Format Analysis
**Tool:** ffprobe → 1.0s
**Result:** H.264 Constrained Baseline, 320×180, 24fps, 596.5s duration, 702 kbps. AAC stereo 48kHz, 65 kbps. Container: MP4/QuickTime.

### Step 2: Black Frame Detection
**Tool:** ffmpeg blackdetect → 3.1s
**Result:** 2 sequences detected:
- 00:00:00.000–00:00:01.250 (1.25s) — Opening black before content
- 09:54:22.000–09:56:27.000 (2.05s) — End credits transition

### Step 3: Audio Loudness (EBU R128)
**Tool:** ffmpeg loudnorm → 18.0s
**Result:** Integrated: -14.2 LUFS | True peak: -2.1 dBTP at 04:32 | Range: 8.3 LU

### Findings:
1. ⚠️ Audio peak at 04:32 — True peak -2.1 dBTP exceeds broadcast limit (-1.0 dBTP). Recommend limiter for JioStar/Hotstar.
2. ℹ️ Resolution mismatch — Source 320×180 (SD) vs delivery targets 1080p/4K. Upscaling will not improve quality.
3. ℹ️ Opening black 1.25s — Within range but trim for OTT delivery.

**Duration:** 22.1s | **Models:** Llama 3.3 70B AWQ | **Tools:** ffprobe, ffmpeg',
metadata_json = json_set(metadata_json, 
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 22.1,
    '$.pipeline.state.attempt', 1)
WHERE board_id = 'c69e29968a2d900158403422596a263354168c5da8a7f0d0e67615c391e47656'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'ingest_and_qc';

-- ── Transcription Step Output ───────────────────────────────
UPDATE notebook_cells SET output = '## Transcription — Whisper Large V3

### Step 1: Audio Extraction
**Tool:** ffmpeg → 0.8s
**Result:** Extracted 16kHz mono WAV (596s, 18.6MB)

### Step 2: Speech-to-Text
**Tool:** whisper large-v3 (CPU) → 312s
**Result:** 12 subtitle segments. Detected language: English (confidence: 0.94)

### Generated SRT:
```
1
00:00:01,250 --> 00:00:05,800
[Gentle orchestral music, birds chirping]

2
00:00:32,100 --> 00:00:38,500
[Butterfly wings flutter, soft wind]

3
00:01:15,000 --> 00:01:22,300
[Dramatic orchestral swell — bunny emerges from burrow]

4
00:02:08,500 --> 00:02:18,200
[Three bullies appear — menacing laughter, cracking knuckles]

5
00:02:45,000 --> 00:02:52,100
[Chase music begins — rapid percussion]

6
00:03:12,000 --> 00:03:18,750
[Trap mechanism — mechanical clicking and spring release]

7
00:04:28,200 --> 00:04:32,800
[LOUD impact — action sequence climax, debris falling]

8
00:05:45,100 --> 00:05:55,300
[Extended chase — rapid footsteps, branches breaking, comedic sound effects]

9
00:07:22,000 --> 00:07:35,500
[Victory fanfare — triumphant orchestral theme]

10
00:08:45,000 --> 00:08:52,000
[Gentle resolution music — birds return]

11
00:09:15,000 --> 00:09:25,000
[Closing theme — full orchestral arrangement]

12
00:09:50,000 --> 00:09:56,200
[End credits — silence]
```

**Note:** Big Buck Bunny is primarily visual with no spoken dialogue. Transcription captures music cues, sound effects, and ambient audio for subtitle/accessibility generation.

**Duration:** 312.8s | **Models:** Whisper Large V3 | **Tools:** ffmpeg, whisper',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 312.8,
    '$.pipeline.state.attempt', 1)
WHERE board_id = 'c69e29968a2d900158403422596a263354168c5da8a7f0d0e67615c391e47656'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'transcription';

-- ── Translation Step Output ─────────────────────────────────
UPDATE notebook_cells SET output = '## Translation — Hindi, Tamil, Telugu, Kannada

### Step 1: Source Analysis
**Tool:** vllm (Llama 3.3 70B) → 8.2s
**Result:** 12 SRT segments analyzed. Content: music/sound descriptions, no spoken dialogue. Cultural adaptation needed for onomatopoeia.

### Step 2: Hindi (हिन्दी)
```
1
00:00:01,250 --> 00:00:05,800
[मधुर वाद्यवृन्द संगीत, चिड़ियों की चहचहाहट]

4
00:02:08,500 --> 00:02:18,200
[तीन बदमाश — धमकी भरी हँसी, उंगलियाँ चटकाना]

7
00:04:28,200 --> 00:04:32,800
[ज़ोरदार टक्कर — एक्शन सीक्वेंस का चरम]

9
00:07:22,000 --> 00:07:35,500
[विजय का संगीत — शानदार वाद्यवृन्द धुन]
```

### Step 3: Tamil (தமிழ்)
```
1
00:00:01,250 --> 00:00:05,800
[மென்மையான இசை, பறவைகள் கீச்சிடுதல்]

4
00:02:08,500 --> 00:02:18,200
[மூன்று அடாவடிகள் — மிரட்டும் சிரிப்பு]

7
00:04:28,200 --> 00:04:32,800
[பெரும் மோதல் — அதிரடி காட்சியின் உச்சம்]
```

### Step 4: Telugu (తెలుగు)
```
1
00:00:01,250 --> 00:00:05,800
[సున్నితమైన సంగీతం, పక్షుల కిలకిల]

7
00:04:28,200 --> 00:04:32,800
[పెద్ద ఢీ — యాక్షన్ సీక్వెన్స్ క్లైమాక్స్]
```

### Step 5: Kannada (ಕನ್ನಡ)
```
1
00:00:01,250 --> 00:00:05,800
[ಮೃದು ಸಂಗೀತ, ಹಕ್ಕಿಗಳ ಚಿಲಿಪಿಲಿ]

7
00:04:28,200 --> 00:04:32,800
[ಭಾರೀ ಡಿಕ್ಕಿ — ಆಕ್ಷನ್ ಕ್ಲೈಮ್ಯಾಕ್ಸ್]
```

### Cultural Notes:
- ⚠️ Sound effect onomatopoeia varies significantly across Dravidian vs Indo-Aryan languages
- ✅ All 4 target languages completed (hi, ta, te, kn)
- ℹ️ Kannada has fewer native speakers in broadcast — consider deprioritizing if budget constrained

**Duration:** 84.6s | **Models:** Llama 3.3 70B AWQ | **Tools:** vllm',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 84.6,
    '$.pipeline.state.attempt', 1)
WHERE board_id = 'c69e29968a2d900158403422596a263354168c5da8a7f0d0e67615c391e47656'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'translation';

-- ── Compliance Step Output ───────────────────────────────────
UPDATE notebook_cells SET output = '## Compliance Scan — CBFC (India), IMDA (Singapore), NMC (UAE)

### Step 1: Regulatory Analysis
**Tool:** vllm (Llama 3.3 70B) → 12.4s
**Result:** Analyzed video content + transcript against territory-specific regulations.

### India — CBFC
**Rating:** U (Universal) ✅
- ✅ No tobacco imagery (COTPA 2003 §5)
- ✅ No alcohol promotion
- ⚠️ **02:08–02:18** — Cartoon bullying scene. Three characters intimidate protagonist. Acceptable under U but borderline for children under 7. CBFC may request advisory card.
- ✅ No religious sensitivity

### Singapore — IMDA
**Rating:** G (General) ✅
- ✅ Content Code compliant
- ✅ No racial/religious content
- ℹ️ Animated slapstick violence — consistent with G

### UAE — NMC
**Rating:** G (General) ✅
- ✅ No content violating Federal Law No. 15/2020
- ✅ No political content
- ⚠️ No Arabic subtitles — required for primary UAE distribution per NMC guidelines
- ✅ Family-friendly

### Findings:
1. ⚠️ **02:08–02:18** — Bullying scene, CBFC borderline U/UA
2. ⚠️ **Missing Arabic subtitles** — Required for UAE market
3. ℹ️ **09:50–09:56** — Verify distributor logos meet territory requirements

**Duration:** 31.2s | **Models:** Llama 3.3 70B AWQ',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 31.2,
    '$.pipeline.state.attempt', 1)
WHERE board_id = 'c69e29968a2d900158403422596a263354168c5da8a7f0d0e67615c391e47656'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'compliance_scan';

-- ── SSAI Step Output ─────────────────────────────────────────
UPDATE notebook_cells SET output = '## SSAI — Ad Insertion Points

### Step 1: Scene Analysis
**Tool:** ffprobe scene detect → 2.1s
**Result:** 23 scene changes across 596s

### Step 2: Engagement Mapping
**Tool:** vllm analysis → 6.3s
**Result:** Mapped engagement curve to timeline. Peak: 0.98 (opening), trough: 0.71 (end credits)

### Recommended Ad Breaks:

| # | Timecode | Type | CPM (₹) | Retention | Revenue/1M | Scene Context |
|---|----------|------|---------|-----------|------------|---------------|
| 1 | 00:00:00 | Pre-roll | 450 | 98% | $4,410 | Before content |
| 2 | 02:45:00 | Mid-roll | 380 | 85% | $3,230 | Scene transition — chase setup complete |
| 3 | 05:52:00 | Mid-roll | 320 | 78% | $2,496 | Post-chase resolution, 1.2s silence |
| 4 | 08:15:00 | Mid-roll | 280 | 72% | $2,016 | Pre-climax buildup |
| 5 | 09:50:00 | Post-roll | 150 | 71% | $1,065 | End credits |

### Revenue Projections:
- **3 breaks (premium):** $10,136/1M views
- **4 breaks (standard):** $12,152/1M views
- **All 5 breaks:** $13,217/1M views

### Platform Rules:
- **JioStar:** Max 3 mid-rolls/10min → Breaks 1, 2, 3
- **YouTube:** Pre-roll + 2 mid-rolls optimal → Breaks 1, 2, 3
- **Hotstar:** ABR dynamic insertion → All breaks available

**Duration:** 28.7s | **Models:** Llama 3.3 70B AWQ | **Tools:** ffprobe, vllm',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 28.7,
    '$.pipeline.state.attempt', 1)
WHERE board_id = 'c69e29968a2d900158403422596a263354168c5da8a7f0d0e67615c391e47656'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'ad_insertion_point_identification';

-- ── Final Review Step Output ─────────────────────────────────
UPDATE notebook_cells SET output = '## Final Review Summary

### Pipeline: 6/6 steps complete

| Step | Duration | Key Output |
|------|----------|------------|
| QC | 22.1s | 3 findings (1 warning: audio peak at 04:32) |
| Transcription | 312.8s | 12 segments, Whisper Large V3, English |
| Translation | 84.6s | 4 languages: hi, ta, te, kn |
| Compliance | 31.2s | CBFC/IMDA/NMC — 1 warning (bullying scene) |
| SSAI | 28.7s | 5 ad breaks, $13.2K/1M projected revenue |
| Final Review | — | This summary |

### Action Items:
1. 🔴 **Audio peak at 04:32** — Apply limiter before broadcast delivery
2. 🟡 **Bullying scene 02:08** — Get CBFC advisory ruling
3. 🟡 **Arabic subtitles** — Add for UAE market compliance
4. 🟢 **Resolution** — Confirm upscaling acceptable for 1080p/4K targets
5. 🟢 **Ad breaks** — Approve 3-break premium config ($10.1K/1M)

### Models Used:
- Llama 3.3 70B AWQ — Analysis, translation, compliance, SSAI
- Whisper Large V3 — Transcription
- ffprobe + ffmpeg — Media analysis

### Total Duration: 479.4s (7m 59s)
### Estimated Cost: $0.04 (GPU compute)',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 0.5,
    '$.pipeline.state.attempt', 1)
WHERE board_id = 'c69e29968a2d900158403422596a263354168c5da8a7f0d0e67615c391e47656'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'final_review';

SQLEOF

echo "   ✅ Pipeline outputs seeded"

# ============================================================================
# Step 2: Seed timecoded notes on video
# ============================================================================
echo ""
echo "📌 Seeding timecoded notes..."

# Clean old notes
sqlite3 "$DB" "DELETE FROM notebook_cells WHERE board_id='$PIPELINE_BOARD' AND cell_type='timecode_note';"

sqlite3 "$DB" << NOTESEOF
-- QC findings as timecoded notes
INSERT OR REPLACE INTO notebook_cells (id, board_id, cell_type, cell_order, content, metadata_json, created_at, updated_at)
VALUES 
('tc_qc_001', '$PIPELINE_BOARD', 'timecode_note', 200, 'Audio peak detected — True peak -2.1 dBTP exceeds broadcast limit (-1.0 dBTP). Apply limiter for JioStar/Hotstar delivery.', '{"timecode_seconds":272.0,"note_type":"qc_issue","severity":"warning","author":"AI/ingest_and_qc","pipeline_step_id":"ingest_and_qc","suggested_action":"Apply audio limiter","ai_reviewed":true,"human_approved":false}', $HOUR_AGO, $HOUR_AGO),

('tc_qc_002', '$PIPELINE_BOARD', 'timecode_note', 201, 'Opening black frame — 1.25s before content starts. Trim for OTT delivery platforms.', '{"timecode_seconds":0.0,"note_type":"qc_issue","severity":"info","author":"AI/ingest_and_qc","pipeline_step_id":"ingest_and_qc","suggested_action":"Trim opening black","ai_reviewed":true,"human_approved":false}', $HOUR_AGO, $HOUR_AGO),

('tc_qc_003', '$PIPELINE_BOARD', 'timecode_note', 202, 'Resolution mismatch — Source is 320×180 SD. Delivery targets include 1080p (JioStar) and 4K (YouTube). Upscaling will degrade quality.', '{"timecode_seconds":1.0,"note_type":"qc_issue","severity":"info","author":"AI/ingest_and_qc","pipeline_step_id":"ingest_and_qc","suggested_action":"Confirm with content team","ai_reviewed":true,"human_approved":false}', $HOUR_AGO, $HOUR_AGO),

-- Compliance findings
('tc_comp_001', '$PIPELINE_BOARD', 'timecode_note', 203, 'Cartoon bullying scene — Three characters intimidate protagonist. CBFC may request U/A advisory card for children under 7.', '{"timecode_seconds":128.5,"note_type":"compliance_issue","severity":"warning","author":"AI/compliance_scan","pipeline_step_id":"compliance_scan","suggested_action":"Request CBFC ruling","ai_reviewed":true,"human_approved":false}', $HOUR_AGO, $HOUR_AGO),

('tc_comp_002', '$PIPELINE_BOARD', 'timecode_note', 204, 'No Arabic subtitles generated — Required for UAE NMC compliance if primary distribution market.', '{"timecode_seconds":590.0,"note_type":"compliance_issue","severity":"warning","author":"AI/compliance_scan","pipeline_step_id":"compliance_scan","suggested_action":"Add Arabic to translation pipeline","ai_reviewed":true,"human_approved":false}', $HOUR_AGO, $HOUR_AGO),

-- SSAI ad break markers
('tc_ssai_001', '$PIPELINE_BOARD', 'timecode_note', 210, 'Pre-roll ad break — Premium slot. CPM ₹450, 98% retention, $4,410/1M views.', '{"timecode_seconds":0.0,"note_type":"ad_break","severity":"info","author":"AI/ad_insertion","pipeline_step_id":"ad_insertion_point_identification","ad_cpm":450,"ad_retention":0.98,"ad_revenue_per_million":4410}', $HOUR_AGO, $HOUR_AGO),

('tc_ssai_002', '$PIPELINE_BOARD', 'timecode_note', 211, 'Mid-roll #1 — Scene transition after chase setup. CPM ₹380, 85% retention, $3,230/1M views.', '{"timecode_seconds":165.0,"note_type":"ad_break","severity":"info","author":"AI/ad_insertion","pipeline_step_id":"ad_insertion_point_identification","ad_cpm":380,"ad_retention":0.85,"ad_revenue_per_million":3230}', $HOUR_AGO, $HOUR_AGO),

('tc_ssai_003', '$PIPELINE_BOARD', 'timecode_note', 212, 'Mid-roll #2 — Post-chase resolution, 1.2s silence gap. CPM ₹320, 78% retention, $2,496/1M views.', '{"timecode_seconds":352.0,"note_type":"ad_break","severity":"info","author":"AI/ad_insertion","pipeline_step_id":"ad_insertion_point_identification","ad_cpm":320,"ad_retention":0.78,"ad_revenue_per_million":2496}', $HOUR_AGO, $HOUR_AGO),

-- Human-added note (shows collaboration)
('tc_human_001', '$PIPELINE_BOARD', 'timecode_note', 220, 'Color looks slightly warm in this scene — check white balance before delivery. @Priya can you verify on the grading monitor?', '{"timecode_seconds":195.0,"note_type":"qc_issue","severity":"info","author":"Rahul","pipeline_step_id":null,"ai_reviewed":false,"human_approved":false}', $TWO_HOURS_AGO, $TWO_HOURS_AGO),

('tc_human_002', '$PIPELINE_BOARD', 'timecode_note', 221, 'Verified on DaVinci — white balance is within spec for Rec.709. No adjustment needed.', '{"timecode_seconds":195.0,"note_type":"qc_issue","severity":"info","author":"Priya","pipeline_step_id":null,"ai_reviewed":false,"human_approved":true,"reply_to":"tc_human_001"}', $HOUR_AGO, $HOUR_AGO);

NOTESEOF

echo "   ✅ Timecoded notes seeded"

# ============================================================================
# Step 3: Summary
# ============================================================================
echo ""
echo "✅ Demo data seeded!"
echo ""
echo "   Pipeline steps: $(sqlite3 "$DB" "SELECT COUNT(*) FROM notebook_cells WHERE board_id='$PIPELINE_BOARD' AND metadata_json LIKE '%pipeline%';")"
echo "   Timecoded notes: $(sqlite3 "$DB" "SELECT COUNT(*) FROM notebook_cells WHERE board_id='$PIPELINE_BOARD' AND cell_type='timecode_note';")"
echo ""
echo "   Restart the Cyan app to see changes."

