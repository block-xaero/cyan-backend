#!/bin/bash
for region in us-west-2 us-east-1; do
  INSTANCE=$(aws ec2 describe-instances \
    --filters "Name=tag:Name,Values=cyan-gpu-worker" "Name=instance-state-name,Values=running" \
    --region $region --query "Reservations[].Instances[].InstanceId" --output text)
  if [ -n "$INSTANCE" ]; then
    aws ec2 terminate-instances --instance-ids $INSTANCE --region $region
    echo "🛑 Terminated $INSTANCE in $region"
  fi
done
