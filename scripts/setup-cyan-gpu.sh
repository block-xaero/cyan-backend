#!/bin/bash
# ============================================================================
# setup-cyan-gpu.sh
#
# Complete setup for Cyan Lens GPU instance (g5.12xlarge)
# Run ONCE after first launch. Everything persists on EBS across stop/start.
#
# What gets installed:
#   - vLLM + Llama 3.1 70B AWQ (translation, compliance, SSAI, QC)
#   - Whisper Large V3 (transcription)
#   - Coqui XTTS v2 (voice dubbing)
#   - ffmpeg + ffprobe (media analysis)
#   - PostgreSQL (via Docker, for Lens graph)
#   - Cyan Lens binary (workflow orchestrator)
#   - All as systemd services (auto-restart on boot)
#
# Usage:
#   scp setup-cyan-gpu.sh ec2-user@<GPU_IP>:/tmp/
#   ssh ec2-user@<GPU_IP> "chmod +x /tmp/setup-cyan-gpu.sh && /tmp/setup-cyan-gpu.sh"
#
# After setup, deploy Cyan Lens binary:
#   scp cyan-lens ec2-user@<GPU_IP>:/opt/cyan/bin/cyan-lens
#   ssh ec2-user@<GPU_IP> "sudo systemctl restart cyan-lens"
# ============================================================================

set -e

echo "═══════════════════════════════════════════════════════════════"
echo "  🚀 Cyan GPU Instance Setup"
echo "  Instance type: g5.12xlarge (4x A10G, 96GB VRAM)"
echo "═══════════════════════════════════════════════════════════════"

# ============================================================================
# 0. Verify GPU
# ============================================================================
echo ""
echo "📺 Step 0: Verifying GPU..."
if ! nvidia-smi > /dev/null 2>&1; then
    echo "❌ nvidia-smi not found. Is this a Deep Learning AMI?"
    echo "   If using plain Amazon Linux, install NVIDIA drivers first."
    exit 1
fi
GPU_COUNT=$(nvidia-smi --query-gpu=name --format=csv,noheader | wc -l)
GPU_MEM=$(nvidia-smi --query-gpu=memory.total --format=csv,noheader | head -1)
echo "✅ Found $GPU_COUNT GPUs ($GPU_MEM each)"

# ============================================================================
# 1. Directory structure
# ============================================================================
echo ""
echo "📁 Step 1: Creating directory structure..."
sudo mkdir -p /opt/cyan/{bin,logs,config,data}
sudo mkdir -p /opt/models/{llama-70b-awq,whisper-large-v3,xtts-v2}
sudo chown -R ec2-user:ec2-user /opt/cyan /opt/models

# ============================================================================
# 2. System packages
# ============================================================================
echo ""
echo "📦 Step 2: Installing system packages..."
sudo yum install -y python3-pip docker git wget 2>/dev/null || \
sudo apt-get update && sudo apt-get install -y python3-pip docker.io git wget 2>/dev/null

# Start Docker for Postgres
sudo systemctl enable docker
sudo systemctl start docker
sudo usermod -aG docker ec2-user

# ============================================================================
# 3. ffmpeg / ffprobe (static build)
# ============================================================================
echo ""
echo "🎬 Step 3: Installing ffmpeg..."
if ! which ffprobe > /dev/null 2>&1; then
    cd /tmp
    wget -q https://johnvansickle.com/ffmpeg/releases/ffmpeg-release-amd64-static.tar.xz
    tar xf ffmpeg-release-amd64-static.tar.xz
    sudo cp ffmpeg-*-amd64-static/ffmpeg /usr/local/bin/
    sudo cp ffmpeg-*-amd64-static/ffprobe /usr/local/bin/
    rm -rf ffmpeg-*
    echo "✅ ffmpeg installed"
else
    echo "✅ ffmpeg already installed"
fi
ffprobe -version | head -1

# ============================================================================
# 4. Python packages
# ============================================================================
echo ""
echo "🐍 Step 4: Installing Python packages..."
pip3 install --upgrade pip --break-system-packages 2>/dev/null || pip3 install --upgrade pip
pip3 install vllm openai-whisper TTS huggingface_hub --break-system-packages 2>/dev/null || \
pip3 install vllm openai-whisper TTS huggingface_hub

# ============================================================================
# 5. Download models (this takes 15-20 minutes)
# ============================================================================
echo ""
echo "🤖 Step 5: Downloading models..."

# Llama 70B AWQ — downloaded automatically by vLLM on first start
# But we can pre-download to /opt/models for reliability
echo "  Downloading Llama 3.1 70B AWQ (this takes ~10 min)..."
python3 -c "
from huggingface_hub import snapshot_download
snapshot_download(
    'TheBloke/Llama-2-70B-Chat-AWQ',
    local_dir='/opt/models/llama-70b-awq',
    local_dir_use_symlinks=False
)
print('  ✅ Llama 70B AWQ downloaded')
" || echo "  ⚠️ Llama download failed — vLLM will download on first start"

# Whisper Large V3 — downloaded on first use, but pre-cache
echo "  Pre-caching Whisper Large V3..."
python3 -c "
import whisper
whisper.load_model('large-v3', download_root='/opt/models/whisper-large-v3')
print('  ✅ Whisper Large V3 cached')
" || echo "  ⚠️ Whisper cache failed — will download on first use"

# XTTS v2 — downloaded by TTS library
echo "  Pre-caching XTTS v2..."
python3 -c "
from TTS.api import TTS
tts = TTS('tts_models/multilingual/multi-dataset/xtts_v2', gpu=False)
print('  ✅ XTTS v2 cached')
" || echo "  ⚠️ XTTS cache failed — will download on first use"

# ============================================================================
# 6. PostgreSQL (Docker)
# ============================================================================
echo ""
echo "🐘 Step 6: Setting up PostgreSQL..."
sudo docker run -d \
    --name cyan-postgres \
    --restart always \
    -e POSTGRES_USER=cyan \
    -e POSTGRES_PASSWORD=cyan \
    -e POSTGRES_DB=cyan_lens \
    -p 5432:5432 \
    -v /opt/cyan/data/postgres:/var/lib/postgresql/data \
    postgres:16-alpine
echo "✅ PostgreSQL running"

# Wait for Postgres to be ready
echo "  Waiting for Postgres..."
sleep 5
for i in $(seq 1 30); do
    if sudo docker exec cyan-postgres pg_isready -U cyan > /dev/null 2>&1; then
        echo "  ✅ Postgres ready"
        break
    fi
    sleep 1
done

# ============================================================================
# 7. Systemd services
# ============================================================================
echo ""
echo "⚙️ Step 7: Creating systemd services..."

# --- vLLM service ---
sudo tee /etc/systemd/system/vllm.service > /dev/null << 'VLLM_EOF'
[Unit]
Description=vLLM - Llama 70B AWQ Inference Server
After=network.target
Wants=network.target

[Service]
Type=simple
User=ec2-user
Environment=CUDA_VISIBLE_DEVICES=0,1,2,3
ExecStart=/usr/local/bin/python3 -m vllm.entrypoints.openai.api_server \
    --model /opt/models/llama-70b-awq \
    --quantization awq \
    --tensor-parallel-size 4 \
    --port 8000 \
    --host 0.0.0.0 \
    --max-model-len 8192
Restart=always
RestartSec=10
StandardOutput=file:/opt/cyan/logs/vllm.log
StandardError=file:/opt/cyan/logs/vllm.log

[Install]
WantedBy=multi-user.target
VLLM_EOF

# --- Whisper API service (simple Flask wrapper) ---
sudo tee /opt/cyan/bin/whisper-server.py > /dev/null << 'WHISPER_EOF'
#!/usr/bin/env python3
"""Simple Whisper API server for Cyan Lens."""
import os, json, tempfile, subprocess
from http.server import HTTPServer, BaseHTTPRequestHandler

WHISPER_MODEL = os.environ.get("WHISPER_MODEL", "large-v3")
WHISPER_CACHE = os.environ.get("WHISPER_CACHE", "/opt/models/whisper-large-v3")

class WhisperHandler(BaseHTTPRequestHandler):
    def do_POST(self):
        if self.path == "/transcribe":
            content_length = int(self.headers.get("Content-Length", 0))
            body = self.rfile.read(content_length)
            
            # Expect JSON with {"audio_url": "..."} or raw audio
            try:
                data = json.loads(body)
                audio_url = data.get("audio_url", "")
                output_format = data.get("format", "srt")
                
                with tempfile.TemporaryDirectory() as tmpdir:
                    audio_path = os.path.join(tmpdir, "audio.wav")
                    
                    # Download or convert audio
                    subprocess.run([
                        "ffmpeg", "-i", audio_url,
                        "-vn", "-ar", "16000", "-ac", "1", "-f", "wav",
                        audio_path
                    ], capture_output=True, timeout=300)
                    
                    # Run Whisper
                    result = subprocess.run([
                        "whisper", audio_path,
                        "--model", WHISPER_MODEL,
                        "--model_dir", WHISPER_CACHE,
                        "--output_format", output_format,
                        "--output_dir", tmpdir
                    ], capture_output=True, text=True, timeout=600)
                    
                    # Read output
                    output_file = os.path.join(tmpdir, f"audio.{output_format}")
                    if os.path.exists(output_file):
                        with open(output_file) as f:
                            transcript = f.read()
                    else:
                        transcript = result.stdout
                    
                    self.send_response(200)
                    self.send_header("Content-Type", "application/json")
                    self.end_headers()
                    self.wfile.write(json.dumps({
                        "transcript": transcript,
                        "format": output_format,
                        "model": WHISPER_MODEL
                    }).encode())
                    
            except Exception as e:
                self.send_response(500)
                self.send_header("Content-Type", "application/json")
                self.end_headers()
                self.wfile.write(json.dumps({"error": str(e)}).encode())
        
        elif self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"OK")
        else:
            self.send_response(404)
            self.end_headers()
    
    def do_GET(self):
        if self.path == "/health":
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"OK")
        else:
            self.send_response(404)
            self.end_headers()

if __name__ == "__main__":
    server = HTTPServer(("0.0.0.0", 8001), WhisperHandler)
    print(f"Whisper API server on :8001 (model: {WHISPER_MODEL})")
    server.serve_forever()
WHISPER_EOF
chmod +x /opt/cyan/bin/whisper-server.py

sudo tee /etc/systemd/system/whisper-api.service > /dev/null << 'WEOF'
[Unit]
Description=Whisper API Server
After=network.target

[Service]
Type=simple
User=ec2-user
Environment=WHISPER_MODEL=large-v3
Environment=WHISPER_CACHE=/opt/models/whisper-large-v3
ExecStart=/usr/bin/python3 /opt/cyan/bin/whisper-server.py
Restart=always
RestartSec=5
StandardOutput=file:/opt/cyan/logs/whisper.log
StandardError=file:/opt/cyan/logs/whisper.log

[Install]
WantedBy=multi-user.target
WEOF

# --- XTTS service ---
sudo tee /etc/systemd/system/xtts.service > /dev/null << 'XTTS_EOF'
[Unit]
Description=Coqui XTTS v2 - Voice Dubbing Server
After=network.target

[Service]
Type=simple
User=ec2-user
Environment=CUDA_VISIBLE_DEVICES=1
ExecStart=/usr/local/bin/python3 -m TTS.server.server \
    --model_name tts_models/multilingual/multi-dataset/xtts_v2 \
    --port 8002 \
    --host 0.0.0.0
Restart=always
RestartSec=10
StandardOutput=file:/opt/cyan/logs/xtts.log
StandardError=file:/opt/cyan/logs/xtts.log

[Install]
WantedBy=multi-user.target
XTTS_EOF

# --- Cyan Lens service ---
sudo tee /etc/systemd/system/cyan-lens.service > /dev/null << 'LENS_EOF'
[Unit]
Description=Cyan Lens - AI Workflow Orchestrator
After=network.target vllm.service
Wants=vllm.service whisper-api.service xtts.service

[Service]
Type=simple
User=ec2-user
Environment=DATABASE_URL=postgresql://cyan:cyan@localhost:5432/cyan_lens
Environment=VLLM_URL=http://127.0.0.1:8000
Environment=VLLM_MODEL=TheBloke/Llama-2-70B-Chat-AWQ
Environment=API_ADDR=0.0.0.0:8080
Environment=RUST_LOG=info
ExecStart=/opt/cyan/bin/cyan-lens
Restart=always
RestartSec=5
StandardOutput=file:/opt/cyan/logs/cyan-lens.log
StandardError=file:/opt/cyan/logs/cyan-lens.log

[Install]
WantedBy=multi-user.target
LENS_EOF

# --- Health check script ---
sudo tee /opt/cyan/bin/health-check.sh > /dev/null << 'HEALTH_EOF'
#!/bin/bash
echo "═══════════════════════════════════════════"
echo "  Cyan GPU Health Check"
echo "═══════════════════════════════════════════"

# GPU
echo -n "GPU:        "
nvidia-smi --query-gpu=utilization.gpu,memory.used,memory.total --format=csv,noheader 2>/dev/null && echo "" || echo "❌"

# vLLM
echo -n "vLLM:       "
curl -sf http://localhost:8000/v1/models > /dev/null && echo "✅ running" || echo "❌ down"

# Whisper
echo -n "Whisper:    "
curl -sf http://localhost:8001/health > /dev/null && echo "✅ running" || echo "❌ down"

# XTTS
echo -n "XTTS:       "
curl -sf http://localhost:8002/ > /dev/null && echo "✅ running" || echo "❌ down"

# Postgres
echo -n "Postgres:   "
docker exec cyan-postgres pg_isready -U cyan > /dev/null 2>&1 && echo "✅ running" || echo "❌ down"

# Cyan Lens
echo -n "Cyan Lens:  "
curl -sf http://localhost:8080/health > /dev/null && echo "✅ running" || echo "❌ down"

# Tools
echo -n "ffprobe:    "
which ffprobe > /dev/null && echo "✅ $(ffprobe -version 2>&1 | head -1)" || echo "❌ not installed"

echo -n "ffmpeg:     "
which ffmpeg > /dev/null && echo "✅ installed" || echo "❌ not installed"

echo ""
echo "Logs:"
echo "  vLLM:      tail -f /opt/cyan/logs/vllm.log"
echo "  Whisper:   tail -f /opt/cyan/logs/whisper.log"
echo "  XTTS:      tail -f /opt/cyan/logs/xtts.log"
echo "  Cyan Lens: tail -f /opt/cyan/logs/cyan-lens.log"
echo ""
echo "Services:"
echo "  sudo systemctl status vllm whisper-api xtts cyan-lens"
HEALTH_EOF
chmod +x /opt/cyan/bin/health-check.sh

# Enable all services
sudo systemctl daemon-reload
sudo systemctl enable vllm whisper-api xtts cyan-lens

# ============================================================================
# 8. Start services (except cyan-lens — needs binary deployed first)
# ============================================================================
echo ""
echo "🚀 Step 8: Starting services..."
sudo systemctl start vllm
echo "  ⏳ vLLM starting (Llama 70B loading... takes 2-3 min)"
sudo systemctl start whisper-api
echo "  ✅ Whisper API started"
sudo systemctl start xtts
echo "  ⏳ XTTS starting (model loading...)"

# ============================================================================
# 9. Config file
# ============================================================================
cat > /opt/cyan/config/env << 'ENVEOF'
# Cyan Lens GPU Instance Configuration
DATABASE_URL=postgresql://cyan:cyan@localhost:5432/cyan_lens
VLLM_URL=http://127.0.0.1:8000
VLLM_MODEL=TheBloke/Llama-2-70B-Chat-AWQ
WHISPER_URL=http://127.0.0.1:8001
XTTS_URL=http://127.0.0.1:8002
API_ADDR=0.0.0.0:8080
RUST_LOG=info
ENVEOF

# ============================================================================
# Done
# ============================================================================
echo ""
echo "═══════════════════════════════════════════════════════════════"
echo "  ✅ Cyan GPU Instance Setup Complete!"
echo "═══════════════════════════════════════════════════════════════"
echo ""
echo "  Next steps:"
echo "  1. Wait 2-3 min for vLLM to load Llama 70B"
echo "  2. SCP cyan-lens binary:"
echo "     scp cyan-lens ec2-user@<IP>:/opt/cyan/bin/cyan-lens"
echo "  3. Start Cyan Lens:"
echo "     sudo systemctl start cyan-lens"
echo "  4. Run health check:"
echo "     /opt/cyan/bin/health-check.sh"
echo ""
echo "  Access from your Mac:"
echo "     ssh -L 8080:localhost:8080 -L 8000:localhost:8000 ec2-user@<IP>"
echo ""
