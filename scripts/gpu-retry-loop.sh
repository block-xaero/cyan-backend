#!/bin/bash
# Retry GPU spot instance every 5 minutes until it works
# Sends text notification on success via terminal-notifier + say

PHONE="your_phone"  # We'll use osascript for notification instead

echo "🔄 GPU spot retry loop started at $(date)"
echo "   Trying every 5 minutes. Will notify on success."
echo "   Press Ctrl+C to stop."
echo ""

ATTEMPT=0

while true; do
  ATTEMPT=$((ATTEMPT + 1))
  echo "━━━ Attempt $ATTEMPT at $(date '+%H:%M:%S') ━━━"
  
  for region in us-west-2 us-east-1; do
    if [ "$region" = "us-west-2" ]; then
      SUBNETS="subnet-0e02197f594887cfb subnet-0a72a9a078aab1884 subnet-086e979b6138abd73"
      AMI="ami-0b8a12f7723681364"
      SG="sg-0d423532ecd69ad31"
      PUB=""
    else
      SUBNETS="subnet-c925ece5 subnet-29fb7e61 subnet-9b75b2c1 subnet-6c615609"
      AMI="ami-0f88e80871fd81e91"
      SG="sg-2e004050"
      PUB="--associate-public-ip-address"
    fi
    
    for subnet in $SUBNETS; do
      RESULT=$(aws ec2 run-instances \
        --image-id $AMI \
        --instance-type g5.12xlarge \
        --key-name cyan-infra \
        --security-group-ids $SG \
        --subnet-id $subnet \
        $PUB \
        --instance-market-options '{"MarketType":"spot","SpotOptions":{"SpotInstanceType":"one-time","InstanceInterruptionBehavior":"terminate"}}' \
        --block-device-mappings '[{"DeviceName":"/dev/xvda","Ebs":{"VolumeSize":200,"VolumeType":"gp3"}}]' \
        --tag-specifications 'ResourceType=instance,Tags=[{Key=Name,Value=cyan-gpu-worker}]' \
        --region $region 2>&1)
      
      if echo "$RESULT" | grep -q "InstanceId"; then
        INSTANCE_ID=$(echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['Instances'][0]['InstanceId'])")
        IP=$(echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['Instances'][0].get('PrivateIpAddress','pending'))")
        
        echo ""
        echo "╔══════════════════════════════════════════╗"
        echo "║  ✅ GPU INSTANCE LAUNCHED!                ║"
        echo "║  Region: $region                          "
        echo "║  Instance: $INSTANCE_ID                   "
        echo "║  IP: $IP                                  "
        echo "║  Attempt: $ATTEMPT                        "
        echo "╚══════════════════════════════════════════╝"
        
        # macOS notifications
        osascript -e 'display notification "Instance: '"$INSTANCE_ID"' IP: '"$IP"'" with title "🚀 GPU LAUNCHED!" sound name "Glass"'
        say "G P U instance launched successfully in $region"
        
        # Also write to a file
        echo "$INSTANCE_ID $IP $region $(date)" > ~/cyan-backend/scripts/gpu-instance.txt
        
        exit 0
      fi
    done
    echo "  ❌ $region: no capacity"
  done
  
  echo "  Sleeping 5 minutes... ($(date '+%H:%M'))"
  sleep 300
done
