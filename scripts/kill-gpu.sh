#!/bin/bash
INSTANCE=$(aws ec2 describe-instances \
  --filters "Name=tag:Name,Values=cyan-gpu-worker" "Name=instance-state-name,Values=running" \
  --region us-west-2 \
  --query "Reservations[].Instances[].InstanceId" --output text)

if [ -n "$INSTANCE" ]; then
  aws ec2 terminate-instances --instance-ids $INSTANCE --region us-west-2
  echo "🛑 Terminated $INSTANCE"
else
  echo "No running gpu-worker found"
fi
