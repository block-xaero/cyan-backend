#!/bin/bash
echo "🚀 Trying g5.12xlarge spot in all AZs..."
for subnet in subnet-0e02197f594887cfb subnet-0a72a9a078aab1884 subnet-086e979b6138abd73; do
  echo "Trying $subnet..."
  RESULT=$(aws ec2 run-instances \
    --image-id ami-0b8a12f7723681364 \
    --instance-type g5.12xlarge \
    --key-name cyan-infra \
    --security-group-ids sg-0d423532ecd69ad31 \
    --subnet-id $subnet \
    --instance-market-options '{"MarketType":"spot","SpotOptions":{"SpotInstanceType":"one-time","InstanceInterruptionBehavior":"terminate"}}' \
    --block-device-mappings '[{"DeviceName":"/dev/xvda","Ebs":{"VolumeSize":200,"VolumeType":"gp3"}}]' \
    --tag-specifications 'ResourceType=instance,Tags=[{Key=Name,Value=cyan-gpu-worker}]' \
    --region us-west-2 2>&1)
  
  if echo "$RESULT" | grep -q "InstanceId"; then
    echo "✅ SUCCESS!"
    echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); i=d['Instances'][0]; print(f'Instance: {i[\"InstanceId\"]}\nIP: {i.get(\"PrivateIpAddress\",\"pending\")}')"
    exit 0
  fi
  echo "❌ No capacity"
done
echo "😞 No luck in us-west-2. Trying us-east-1..."
for subnet in subnet-c925ece5 subnet-29fb7e61 subnet-9b75b2c1 subnet-6c615609; do
  echo "Trying us-east-1 $subnet..."
  RESULT=$(aws ec2 run-instances \
    --image-id ami-0f88e80871fd81e91 \
    --instance-type g5.12xlarge \
    --key-name cyan-infra \
    --security-group-ids sg-2e004050 \
    --subnet-id $subnet \
    --associate-public-ip-address \
    --instance-market-options '{"MarketType":"spot","SpotOptions":{"SpotInstanceType":"one-time","InstanceInterruptionBehavior":"terminate"}}' \
    --block-device-mappings '[{"DeviceName":"/dev/xvda","Ebs":{"VolumeSize":200,"VolumeType":"gp3"}}]' \
    --tag-specifications 'ResourceType=instance,Tags=[{Key=Name,Value=cyan-gpu-worker}]' \
    --region us-east-1 2>&1)
  
  if echo "$RESULT" | grep -q "InstanceId"; then
    echo "✅ SUCCESS in us-east-1!"
    echo "$RESULT" | python3 -c "import sys,json; d=json.load(sys.stdin); i=d['Instances'][0]; print(f'Instance: {i[\"InstanceId\"]}\nIP: {i.get(\"PrivateIpAddress\",\"pending\")}\nPublicIP: {i.get(\"PublicIpAddress\",\"pending\")}')"
    exit 0
  fi
  echo "❌ No capacity"
done
echo "😞 No luck anywhere. Try again later."
