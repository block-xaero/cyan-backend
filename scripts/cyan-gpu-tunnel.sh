#!/bin/bash
# ============================================================================
# cyan-gpu-tunnel.sh
#
# Opens SSH tunnels to the Cyan GPU instance through bastion.
# All Lens + model APIs accessible on localhost.
#
# Usage:
#   ./cyan-gpu-tunnel.sh          # start tunnels
#   ./cyan-gpu-tunnel.sh status   # check if tunnels are up
#   ./cyan-gpu-tunnel.sh stop     # kill tunnels
#   ./cyan-gpu-tunnel.sh health   # remote health check
# ============================================================================

KEY="${HOME}/.ssh/cyan-infra.pem"
BASTION="34.214.5.244"
BASTION_USER="ec2-user"

# GPU instance IP — UPDATE THIS after launch
GPU_IP="${CYAN_GPU_IP:-10.0.48.183}"

if [ "$GPU_IP" = "10.0.48.183" ]; then
    echo "❌ Set GPU_IP first:"
    echo "   export CYAN_GPU_IP=10.0.X.X"
    echo "   or edit this script"
    exit 1
fi

case "${1:-start}" in
    start)
        echo "🔗 Opening tunnels to GPU instance ($GPU_IP)..."
        
        # Kill existing tunnels
        pkill -f "ssh.*cyan-gpu-tunnel" 2>/dev/null
        sleep 1
        
        # Open tunnels
        ssh -i "$KEY" \
            -o ServerAliveInterval=30 \
            -o ServerAliveCountMax=3 \
            -o ExitOnForwardFailure=yes \
            -L 8080:$GPU_IP:8080 \
            -L 8000:$GPU_IP:8000 \
            -L 8001:$GPU_IP:8001 \
            -L 8002:$GPU_IP:8002 \
            -L 5432:$GPU_IP:5432 \
            -N \
            -f \
            -o "Tag=cyan-gpu-tunnel" \
            ${BASTION_USER}@${BASTION}
        
        sleep 2
        
        echo "  Cyan Lens:  http://localhost:8080"
        echo "  vLLM:       http://localhost:8000"
        echo "  Whisper:    http://localhost:8001"
        echo "  XTTS:       http://localhost:8002"
        echo "  Postgres:   localhost:5432"
        echo ""
        
        # Quick check
        echo -n "  Lens health: "
        curl -sf http://localhost:8080/health && echo "" || echo "❌ (may still be starting)"
        echo -n "  vLLM health: "
        curl -sf http://localhost:8000/v1/models > /dev/null && echo "✅" || echo "⏳ loading model..."
        ;;
    
    status)
        echo "🔍 Tunnel status:"
        if pgrep -f "ssh.*cyan-gpu-tunnel" > /dev/null 2>/dev/null; then
            # Fallback: check if ports are forwarded
            if pgrep -f "ssh.*8080.*${BASTION}" > /dev/null 2>/dev/null; then
                echo "  ✅ Tunnels active"
                echo -n "  Lens:  "; curl -sf http://localhost:8080/health || echo "❌"
                echo -n "  vLLM:  "; curl -sf http://localhost:8000/v1/models > /dev/null && echo "✅" || echo "❌"
            else
                echo "  ❌ Tunnels not found"
            fi
        else
            # Check by port
            if lsof -i :8080 > /dev/null 2>&1; then
                echo "  ✅ Port 8080 forwarded"
            else
                echo "  ❌ No tunnels running"
                echo "  Run: $0 start"
            fi
        fi
        ;;
    
    stop)
        echo "🛑 Stopping tunnels..."
        pkill -f "ssh.*${BASTION}.*8080" 2>/dev/null
        pkill -f "ssh.*cyan-gpu-tunnel" 2>/dev/null
        echo "  ✅ Tunnels stopped"
        ;;
    
    health)
        echo "🏥 Remote health check..."
        ssh -i "$KEY" -o ProxyJump=${BASTION_USER}@${BASTION} ec2-user@${GPU_IP} \
            "/opt/cyan/bin/health-check.sh"
        ;;
    
    ssh)
        echo "🖥️  Connecting to GPU instance..."
        ssh -i "$KEY" -o ProxyJump=${BASTION_USER}@${BASTION} ec2-user@${GPU_IP}
        ;;
    
    deploy)
        echo "🚀 Deploying Cyan Lens binary..."
        cd ~/cyan-lens
        scp -i "$KEY" -o ProxyJump=${BASTION_USER}@${BASTION} \
            target/release/cyan-lens ec2-user@${GPU_IP}:/tmp/cyan-lens
        ssh -i "$KEY" -o ProxyJump=${BASTION_USER}@${BASTION} ec2-user@${GPU_IP} \
            "sudo mv /tmp/cyan-lens /opt/cyan/bin/cyan-lens && sudo chmod +x /opt/cyan/bin/cyan-lens && sudo systemctl restart cyan-lens && sleep 2 && sudo systemctl status cyan-lens --no-pager"
        ;;
    
    logs)
        SERVICE="${2:-cyan-lens}"
        echo "📋 Logs for $SERVICE..."
        ssh -i "$KEY" -o ProxyJump=${BASTION_USER}@${BASTION} ec2-user@${GPU_IP} \
            "tail -50 /opt/cyan/logs/${SERVICE}.log"
        ;;

    *)
        echo "Usage: $0 {start|status|stop|health|ssh|deploy|logs [service]}"
        ;;
esac
