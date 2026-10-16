#!/bin/sh
# wait up to 90 min for AC and 1-min load <= 4; print state
for i in $(seq 1 45); do
  ac=$(pmset -g batt | head -1 | grep -c "AC Power"); l=$(sysctl -n vm.loadavg | awk '{print $2}')
  if [ "$ac" = 1 ] && [ $(echo "$l <= 4" | bc) = 1 ]; then echo "gate ok load=$l"; exit 0; fi
  sleep 120
done; echo "gate timeout load=$l ac=$ac"; exit 1
