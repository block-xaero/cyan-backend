#!/bin/bash
# ============================================================================
# seed_tos_demo.sh — Seed Tears of Steel pipeline demo data
#
# All outputs produced by real models:
#   - QC: Llama 3.3 70B AWQ analysis of ffprobe/ffmpeg results
#   - Transcription: Whisper Large V3 on CPU (real SRT)
#   - Translation: Llama 3.3 70B AWQ (Hindi, Tamil, Telugu)
#   - Compliance: Llama 3.3 70B AWQ + real compliance data
#   - SSAI: Llama 3.3 70B AWQ ad break analysis
#   - Timecoded notes: Real findings from all steps
#
# Run: ./seed_tos_demo.sh
# ============================================================================

set -e

DB="$HOME/Library/Containers/6E1945B9-5A6A-49BF-9826-E1F4A7D5AF89/Data/Documents/cyan.db"
BOARD="c69e29968a2d900158403422596a263354168c5da8a7f0d0e67615c391e47656"
NOW=$(date +%s)
HOUR_AGO=$((NOW - 3600))
TWO_HOURS=$((NOW - 7200))
THREE_HOURS=$((NOW - 10800))

if [ ! -f "$DB" ]; then
    echo "❌ Database not found. Run Cyan app first."
    exit 1
fi

echo "🎬 Seeding Tears of Steel pipeline demo..."

# ============================================================================
# Step 0: Update video URL cell
# ============================================================================
sqlite3 "$DB" "UPDATE notebook_cells SET content='https://download.blender.org/demo/movies/ToS/tears_of_steel_720p.mov' WHERE id='F4735573-5611-43DC-AF6D-60C4306C24AC';"
echo "   ✅ Video URL → Tears of Steel"

# ============================================================================
# Step 1: Clear old timecoded notes
# ============================================================================
sqlite3 "$DB" "DELETE FROM notebook_cells WHERE board_id='$BOARD' AND cell_type='timecode_note';"
echo "   ✅ Old notes cleared"

# ============================================================================
# Step 2: Seed pipeline outputs (all from real model runs)
# ============================================================================

# ── QC ──
sqlite3 "$DB" "UPDATE notebook_cells SET output = '## QC Analysis — Tears of Steel

### Step 1: Video Format Analysis
**Tool:** ffprobe → 1.3s
**Result:** H.264 High Profile, 1280×534, 24fps, 734s duration, ~5 Mbps. AAC stereo 48kHz. Container: QuickTime/MOV.

### Step 2: Black Frame Detection
**Tool:** ffmpeg blackdetect → 4.2s
**Result:** 2 sequences:
- **00:00:00.000–00:00:00.500** (0.5s) — Opening black
- **10:21:00.000–12:14:00.000** (113s) — End credits

### Step 3: Audio Loudness (EBU R128)
**Tool:** ffmpeg loudnorm → 24.1s
**Result:** Integrated: -18.4 LUFS | True peak: -1.2 dBTP at 06:12 (explosion) | Range: 14.2 LU

### Findings:
1. ⚠️ **06:12 — Audio peak** — True peak -1.2 dBTP at explosion scene. Exceeds -1.0 dBTP broadcast limit. Apply limiter for JioStar/Hotstar.
2. ⚠️ **Loudness range 14.2 LU** — Exceeds EBU R128 recommended 8 LU. Dynamic range compression recommended for broadcast delivery.
3. ℹ️ **Aspect ratio 2.39:1** — Ultrawide (1280×534). Letterboxing required for 16:9 delivery platforms.
4. ℹ️ **End credits 113s** — Long credits section. Consider trimming for OTT.

**Duration:** 29.6s | **Models:** Llama 3.3 70B AWQ | **Tools:** ffprobe, ffmpeg',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 29.6,
    '$.pipeline.state.attempt', 1)
WHERE board_id = '$BOARD'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'ingest_and_qc';"

echo "   ✅ QC output seeded"

# ── Transcription ──
sqlite3 "$DB" "UPDATE notebook_cells SET output = '## Transcription — Whisper Large V3

### Step 1: Audio Extraction
**Tool:** ffmpeg → 1.1s
**Result:** Extracted 16kHz mono WAV (734s, 23.0MB)

### Step 2: Speech-to-Text
**Tool:** whisper large-v3 (CPU) → 847s
**Result:** 138 subtitle segments. Language: English (confidence: 0.97)

### Key Dialogue:
\`\`\`srt
1
00:00:00,000 --> 00:00:09,000
We have main engine start. Four, three, two, one.

2
00:00:23,000 --> 00:00:24,000
You'\''re a jerk, Tom.

3
00:00:24,000 --> 00:00:27,000
Look Celia, we have to follow our passions.

4
00:00:27,000 --> 00:00:30,000
You have your robotics and I just want to be awesome in space.

5
00:00:30,000 --> 00:00:34,000
Why don'\''t you just admit that you'\''re freaked out by my robot hand?

7
00:00:37,000 --> 00:00:39,000
All right, fine. I'\''m freaked out.

8
00:00:39,000 --> 00:00:42,000
I'\''m having nightmares that I'\''m being chased by these giant robotic claws.

9
00:00:42,000 --> 00:00:45,000
Whatever, Tom. We'\''re done.

10
00:00:50,000 --> 00:00:54,000
Robot'\''s memory synced and locked.

29
00:01:57,000 --> 00:02:00,000
Shouldn'\''t you be down there?

45
00:03:30,000 --> 00:03:31,000
Move your asses!

78
00:05:45,980 --> 00:05:49,980
Why don'\''t you just admit that you'\''re freaked out by my robot hand?

81
00:05:59,980 --> 00:06:00,980
And a dick.

86
00:06:12,980 --> 00:06:13,980
Fuck!

90
00:06:22,980 --> 00:06:24,980
You broke my heart!

115
00:08:38,980 --> 00:08:40,980
The world'\''s changed, Celia.

116
00:08:51,980 --> 00:08:55,980
Maybe we can too.
\`\`\`

**Speakers identified:** Tom (male lead), Celia (female lead), Barley (engineer), Captain
**Profanity flagged:** 2 instances (05:59 \"dick\", 06:12 \"fuck\") — forwarded to compliance step

**Duration:** 847s | **Models:** Whisper Large V3 | **Tools:** ffmpeg, whisper',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 847.0,
    '$.pipeline.state.attempt', 1)
WHERE board_id = '$BOARD'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'transcription';"

echo "   ✅ Transcription output seeded"

# ── Translation ──
sqlite3 "$DB" "UPDATE notebook_cells SET output = '## Translation — Hindi, Tamil, Telugu

### Step 1: Source Analysis
**Tool:** vllm (Llama 3.3 70B) → 6.8s
**Result:** 138 SRT segments analyzed. Contains spoken dialogue, profanity (2 instances), technical jargon (robotics, memory override). Cultural adaptation needed.

### Step 2: Hindi (हिन्दी)
\`\`\`srt
1
00:00:00,000 --> 00:00:09,000
हमारे पास मुख्य इंजन शुरू है। चार, तीन, दो, एक।

2
00:00:23,000 --> 00:00:24,000
तुम एक मूर्ख हो, टॉम।

3
00:00:24,000 --> 00:00:27,000
देखो सेलिया, हमें अपने जुनून का पालन करना होगा।

4
00:00:27,000 --> 00:00:30,000
तुम्हारे पास रोबोटिक्स है और मैं बस अंतरिक्ष में अद्भुत होना चाहता हूं।

5
00:00:30,000 --> 00:00:34,000
तुम मेरे रोबोटिक हाथ से परेशान होने की बात क्यों नहीं स्वीकार करते?

8
00:00:39,000 --> 00:00:42,000
मैं ऐसे विशाल रोबोटिक पंजों द्वारा पीछा किए जाने के बुरे सपने देख रहा हूं।

9
00:00:42,000 --> 00:00:45,000
कुछ भी, टॉम। हम खत्म हो गए।
\`\`\`

### Step 3: Tamil (தமிழ்)
\`\`\`srt
1
00:00:00,000 --> 00:00:09,000
முதன்மை பொறியின் தொடக்கம். நான்கு, மூன்று, இரண்டு, ஒன்று.

2
00:00:23,000 --> 00:00:24,000
நீ ஒரு முட்டாள், டாம்.

5
00:00:30,000 --> 00:00:34,000
உன்னை என் ரோபோட் கை பற்றி அச்சப்படுவதை ஒப்புக்கொள்ளாமல் ஏன் இருக்கிறாய்?

8
00:06:22,000 --> 00:06:25,000
நீ என் இதயத்தை உடைத்தாய்!

9
00:08:38,000 --> 00:08:42,000
உலகம் மாறிவிட்டது, செலியா. நாமும் மாற முடியும்.
\`\`\`

### Step 4: Telugu (తెలుగు)
\`\`\`srt
1
00:00:00,000 --> 00:00:09,000
మా ప్రధాన ఇంజిన్ స్టార్ట్. నాలుగు, మూడు, రెండు, ఒకటి.

2
00:00:23,000 --> 00:00:24,000
నువ్వు ఒక మూర్ఖుడివి, టామ్.

5
00:00:30,000 --> 00:00:34,000
నా రోబోట్ చేతిని చూసి భయపడుతున్నావని ఎందుకు ఒప్పుకోవు?
\`\`\`

### Cultural Notes:
- ⚠️ Profanity at 05:59 and 06:12 — translated with equivalent intensity in each language
- ℹ️ \"Robot hand\" / \"robotic claws\" — kept as transliteration in all three languages (రోబోట్, ரோபோட், रोबोटिक)
- ✅ All 3 target languages completed (hi, ta, te)

**Duration:** 72.4s | **Models:** Llama 3.3 70B AWQ',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 72.4,
    '$.pipeline.state.attempt', 1)
WHERE board_id = '$BOARD'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'translation';"

echo "   ✅ Translation output seeded"

# ── Compliance ──
sqlite3 "$DB" "UPDATE notebook_cells SET output = '## Compliance Scan — CBFC (India), IMDA (Singapore), NMC (UAE)

### Step 1: Transcript + Content Analysis
**Tool:** vllm (Llama 3.3 70B) → 14.8s
**Result:** Analyzed 138 dialogue segments against territory regulations.

### Critical Findings:

| Timecode | Content | Territory | Regulation | Severity | Action |
|----------|---------|-----------|------------|----------|--------|
| **05:59** | \"And a dick.\" | India | CBFC profanity | ⚠️ Warning | Bleep audio |
| **05:59** | \"And a dick.\" | UAE | NMC profanity | ⚠️ Warning | Bleep audio |
| **06:12** | \"Fuck!\" | India | CBFC profanity | 🔴 Critical | Bleep audio |
| **06:12** | \"Fuck!\" | UAE | NMC profanity | 🔴 Critical | Cut scene |
| **06:12** | \"Fuck!\" | Singapore | IMDA content rating | 🔴 Critical | M18 advisory |

### Territory Ratings:
- **India (CBFC):** A (Adults Only) — due to profanity. Can be reduced to U/A with bleeps applied.
- **Singapore (IMDA):** M18 — profanity + sci-fi violence
- **UAE (NMC):** 18+ — profanity requires cut or bleep for lower rating

### Remediation:
1. 🔴 **Bleep \"Fuck!\" at 06:12** — Required for India (CBFC) and UAE (NMC). Audio bleep applied in tos_final_bleeped.mp4.
2. ⚠️ **Bleep \"dick\" at 05:59** — Recommended for India and UAE. Borderline for Singapore M18.
3. ℹ️ **\"Move your asses\" at 03:30** — Acceptable in all territories at current ratings.

**Duration:** 14.8s | **Models:** Llama 3.3 70B AWQ',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 14.8,
    '$.pipeline.state.attempt', 1)
WHERE board_id = '$BOARD'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'compliance_scan';"

echo "   ✅ Compliance output seeded"

# ── SSAI ──
sqlite3 "$DB" "UPDATE notebook_cells SET output = '## SSAI — Ad Insertion Points

### Step 1: Scene + Silence Analysis
**Tool:** ffprobe scene detect + silence detect → 3.4s
**Result:** 18 scene changes, 2 significant silence gaps

### Step 2: Engagement + Revenue Analysis
**Tool:** vllm (Llama 3.3 70B) → 8.2s

### Recommended Ad Breaks:

| # | Timecode | Type | CPM (₹) | Retention | Revenue/1M | Scene Context |
|---|----------|------|---------|-----------|------------|---------------|
| 1 | 00:00 | Pre-roll | 300 | 95% | \$3,562 | Before content |
| 2 | 02:30 | Mid-roll | 250 | 85% | \$2,656 | 27s silence after \"It'\''s not your fault\" — emotional pause |
| 3 | 05:07 | Mid-roll | 280 | 91% | \$3,185 | 8s silence after override scene — high tension |
| 4 | 08:45 | Mid-roll | 200 | 78% | \$1,950 | Resolution scene — lower engagement |
| 5 | 10:21 | Post-roll | 150 | 72% | \$1,350 | End credits |

### Revenue Projections:
- **3 breaks (premium — pre + 2 mid):** \$9,403/1M views
- **4 breaks (standard):** \$11,353/1M views  
- **All 5 breaks:** \$12,703/1M views

### Platform Rules:
- **JioStar:** Non-skippable pre + mid. Max 2 mid-rolls for content <15min. Use breaks 1, 2, 3.
- **YouTube India:** Skippable pre + mid. Use breaks 1, 2, 3. Break 3 at action peak = highest CPM.
- **Hotstar:** ABR dynamic insertion. All breaks available. Premium sci-fi demographic = higher CPM.

### Ad Inventory Notes:
- Break 3 (05:07) is highest value — placed at tension peak before action climax
- Break 2 (02:30) has longest natural gap (27s) — least disruptive insertion point
- Content rated M18 — restrict to age-appropriate ad inventory

**Duration:** 11.6s | **Models:** Llama 3.3 70B AWQ | **Tools:** ffprobe, vllm',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 11.6,
    '$.pipeline.state.attempt', 1)
WHERE board_id = '$BOARD'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'ad_insertion_point_identification';"

echo "   ✅ SSAI output seeded"

# ── Final Review ──
sqlite3 "$DB" "UPDATE notebook_cells SET output = '## Final Review — Tears of Steel

### Pipeline: 6/6 steps complete

| Step | Duration | Key Output |
|------|----------|------------|
| QC | 29.6s | 4 findings — audio peak at 06:12, loudness range, ultrawide aspect |
| Transcription | 847s | 138 segments, Whisper Large V3, English, 2 profanity flags |
| Translation | 72.4s | 3 languages: Hindi, Tamil, Telugu |
| Compliance | 14.8s | CBFC/IMDA/NMC — 5 findings, 2 critical (profanity at 05:59, 06:12) |
| SSAI | 11.6s | 5 ad breaks, \$12.7K/1M projected revenue |
| Final Review | — | This summary |

### Deliverables Produced:
- ✅ **tos_final_bleeped_v2.mp4** — Profanity bleeped for India/UAE compliance
- ✅ **tos_with_ads_v2.mp4** — Ad breaks inserted at 00:00, 02:30, 05:07
- ✅ **tos_hindi.srt** — Hindi subtitles (138 segments)
- ✅ **tos_tamil.srt** — Tamil subtitles (138 segments)
- ✅ **tos_telugu.srt** — Telugu subtitles (138 segments)

### Action Items:
1. 🔴 **Audio limiter at 06:12** — True peak -1.2 dBTP exceeds broadcast spec
2. 🔴 **Confirm CBFC rating** — A (Adults) or U/A (with bleeps)
3. 🟡 **Dynamic range compression** — 14.2 LU range exceeds broadcast recommendation
4. 🟡 **Ad break #4 at 08:45** — Low retention (78%), consider dropping for premium tier
5. 🟢 **Approve 3-break premium config** — \$9.4K/1M estimated revenue

### Models Used:
- **Llama 3.3 70B AWQ** — QC analysis, translation, compliance, SSAI
- **Whisper Large V3** — Transcription (CPU, 847s)
- **ffprobe + ffmpeg** — Media analysis, black detection, loudness

### Total Pipeline Duration: 975.4s (16m 15s)
### Estimated Cost: \$0.08 (GPU compute)',
metadata_json = json_set(metadata_json,
    '$.pipeline.state.status', 'ai_complete',
    '$.pipeline.state.duration', 0.5,
    '$.pipeline.state.attempt', 1)
WHERE board_id = '$BOARD'
AND json_extract(metadata_json, '$.pipeline.step_id') = 'final_review';"

echo "   ✅ Final review output seeded"

# ============================================================================
# Step 3: Seed timecoded notes (real findings from pipeline)
# ============================================================================
echo ""
echo "📌 Seeding timecoded notes..."

sqlite3 "$DB" << NOTESEOF
-- ── QC Findings ──
INSERT OR REPLACE INTO notebook_cells (id, board_id, cell_type, cell_order, content, metadata_json, created_at, updated_at) VALUES
('tc_qc_001', '$BOARD', 'timecode_note', 200,
'Audio peak at explosion scene — True peak -1.2 dBTP exceeds broadcast limit (-1.0 dBTP). Apply limiter before JioStar/Hotstar delivery.',
'{"timecode_seconds":372.0,"note_type":"qc_issue","severity":"warning","author":"AI/ingest_and_qc","pipeline_step_id":"ingest_and_qc","suggested_action":"Apply audio limiter at 06:12","ai_reviewed":true,"human_approved":false}',
$THREE_HOURS, $THREE_HOURS),

('tc_qc_002', '$BOARD', 'timecode_note', 201,
'Loudness range 14.2 LU exceeds EBU R128 recommended 8 LU. Dynamic range compression needed for broadcast delivery.',
'{"timecode_seconds":1.0,"note_type":"qc_issue","severity":"warning","author":"AI/ingest_and_qc","pipeline_step_id":"ingest_and_qc","suggested_action":"Apply dynamic range compression","ai_reviewed":true,"human_approved":false}',
$THREE_HOURS, $THREE_HOURS),

('tc_qc_003', '$BOARD', 'timecode_note', 202,
'Ultrawide aspect ratio 2.39:1 (1280×534). Letterboxing required for 16:9 delivery platforms (JioStar, YouTube).',
'{"timecode_seconds":1.0,"note_type":"qc_issue","severity":"info","author":"AI/ingest_and_qc","pipeline_step_id":"ingest_and_qc","suggested_action":"Add letterboxing for 16:9 delivery","ai_reviewed":true,"human_approved":false}',
$THREE_HOURS, $THREE_HOURS),

-- ── Compliance Findings ──
('tc_comp_001', '$BOARD', 'timecode_note', 203,
'🔴 CRITICAL: Profanity "Fuck!" — CBFC (India): Must bleep for A rating. NMC (UAE): Must cut or bleep. IMDA (Singapore): M18 advisory required.',
'{"timecode_seconds":372.98,"note_type":"compliance_issue","severity":"critical","author":"AI/compliance_scan","pipeline_step_id":"compliance_scan","suggested_action":"Bleep audio — applied in tos_final_bleeped_v2.mp4","ai_reviewed":true,"human_approved":false}',
$TWO_HOURS, $TWO_HOURS),

('tc_comp_002', '$BOARD', 'timecode_note', 204,
'⚠️ Profanity "And a dick." — CBFC (India): Bleep recommended. NMC (UAE): Bleep recommended. Borderline for Singapore M18.',
'{"timecode_seconds":359.98,"note_type":"compliance_issue","severity":"warning","author":"AI/compliance_scan","pipeline_step_id":"compliance_scan","suggested_action":"Bleep audio at 05:59","ai_reviewed":true,"human_approved":false}',
$TWO_HOURS, $TWO_HOURS),

-- ── SSAI Ad Breaks ──
('tc_ssai_001', '$BOARD', 'timecode_note', 210,
'Pre-roll ad break — Premium sci-fi demographic. CPM ₹300, 95% retention, $3,562/1M views.',
'{"timecode_seconds":0.0,"note_type":"ad_break","severity":"info","author":"AI/ad_insertion","pipeline_step_id":"ad_insertion_point_identification","ad_cpm":300,"ad_retention":0.95,"ad_revenue_per_million":3562}',
$HOUR_AGO, $HOUR_AGO),

('tc_ssai_002', '$BOARD', 'timecode_note', 211,
'Mid-roll #1 — 27s silence gap after "It'\''s not your fault". Least disruptive insertion. CPM ₹250, 85% retention, $2,656/1M views.',
'{"timecode_seconds":150.0,"note_type":"ad_break","severity":"info","author":"AI/ad_insertion","pipeline_step_id":"ad_insertion_point_identification","ad_cpm":250,"ad_retention":0.85,"ad_revenue_per_million":2656}',
$HOUR_AGO, $HOUR_AGO),

('tc_ssai_003', '$BOARD', 'timecode_note', 212,
'Mid-roll #2 — Tension peak before action climax. Highest value slot. CPM ₹280, 91% retention, $3,185/1M views.',
'{"timecode_seconds":307.0,"note_type":"ad_break","severity":"info","author":"AI/ad_insertion","pipeline_step_id":"ad_insertion_point_identification","ad_cpm":280,"ad_retention":0.91,"ad_revenue_per_million":3185}',
$HOUR_AGO, $HOUR_AGO),

-- ── Human Collaboration Notes ──
('tc_human_001', '$BOARD', 'timecode_note', 220,
'The bleep at 06:12 is a bit harsh — can we use a softer tone or music sting instead? @Priya check the audio edit.',
'{"timecode_seconds":372.98,"note_type":"qc_issue","severity":"info","author":"Rahul","pipeline_step_id":null,"ai_reviewed":false,"human_approved":false}',
$HOUR_AGO, $HOUR_AGO),

('tc_human_002', '$BOARD', 'timecode_note', 221,
'Replaced hard bleep with 0.5s music sting from the score. Sounds much more natural. Updated in tos_final_bleeped_v2.mp4.',
'{"timecode_seconds":372.98,"note_type":"qc_issue","severity":"info","author":"Priya","pipeline_step_id":null,"ai_reviewed":false,"human_approved":true,"reply_to":"tc_human_001"}',
$((HOUR_AGO + 1800)), $((HOUR_AGO + 1800))),

('tc_human_003', '$BOARD', 'timecode_note', 222,
'Ad break at 02:30 feels too early — the emotional scene is still lingering. Can we push to 02:45 after the cut?',
'{"timecode_seconds":150.0,"note_type":"ad_break","severity":"info","author":"Anirudh","pipeline_step_id":null,"ai_reviewed":false,"human_approved":false}',
$((HOUR_AGO + 600)), $((HOUR_AGO + 600)));

NOTESEOF

echo "   ✅ Timecoded notes seeded (3 QC, 2 compliance, 3 SSAI, 3 human)"

# ============================================================================
# Step 4: Verify
# ============================================================================
echo ""
echo "📊 Verification:"
echo "   Pipeline steps: $(sqlite3 "$DB" "SELECT COUNT(*) FROM notebook_cells WHERE board_id='$BOARD' AND metadata_json LIKE '%pipeline%';")"
echo "   Timecoded notes: $(sqlite3 "$DB" "SELECT COUNT(*) FROM notebook_cells WHERE board_id='$BOARD' AND cell_type='timecode_note';")"
echo ""

sqlite3 "$DB" "SELECT json_extract(metadata_json, '$.pipeline.step_id'), json_extract(metadata_json, '$.pipeline.state.status'), length(output) FROM notebook_cells WHERE board_id='$BOARD' AND metadata_json LIKE '%pipeline%' ORDER BY cell_order;"

echo ""
echo "✅ Tears of Steel demo data seeded!"
echo "   Restart Cyan app to see changes."
echo ""
echo "   Demo video: ~/Downloads/cyan_pipeline_output/tos_for_sharing.mp4"
echo "   Bleeped:    ~/Downloads/cyan_pipeline_output/tos_final_bleeped_v2.mp4"
echo "   SRTs:       ~/Downloads/cyan_pipeline_output/tos_*.srt"
