#!/bin/bash
AMI="ami-0710b552fb40d320b"
SUBNETS=("subnet-0e02197f594887cfb" "subnet-020d4542f623c27fa" "subnet-086e979b6138abd73")

echo "🔄 g5.12xlarge retry — every 2 min across all AZs"
echo "   $(date). Leave running."

ATTEMPT=0
while true; do
    ATTEMPT=$((ATTEMPT + 1))
    for SUBNET in "${SUBNETS[@]}"; do
        RESULT=$(aws ec2 run-instances \
            --image-id $AMI \
            --instance-type g5.12xlarge \
            --key-name cyan-infra \
            --security-group-ids sg-0d423532ecd69ad31 \
            --subnet-id $SUBNET \
            --block-device-mappings '[{"DeviceName":"/dev/xvda","Ebs":{"VolumeSize":200,"VolumeType":"gp3"}}]' \
            --tag-specifications 'ResourceType=instance,Tags=[{Key=Name,Value=cyan-gpu-worker}]' \
            --region us-west-2 2>&1)
        
        if echo "$RESULT" | grep -q "InstanceId"; then
            IP=$(echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['Instances'][0].get('PrivateIpAddress','pending'))")
            ID=$(echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d['Instances'][0]['InstanceId'])")
            echo ""
            echo "✅✅✅ g5.12xlarge LAUNCHED! Instance: $ID IP: $IP ✅✅✅"
            osascript -e "display notification \"g5.12xlarge IP: $IP\" with title \"🚀 GPU LAUNCHED\" sound name \"Glass\"" 2>/dev/null
            say "G 5 12 x large launched successfully" 2>/dev/null
            echo "$ID $IP" > ~/cyan-backend/scripts/gpu-instance.txt
            exit 0
        fi
    done
    echo "  Attempt $ATTEMPT — no capacity $(date '+%H:%M')"
    sleep 120
done
