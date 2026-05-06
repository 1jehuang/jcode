---
name: network-traffic-drops
description: Troubleshoot packet drops on gateway, compute, bond, tap, NIC, softnet, OVS, or OVN networking paths. Use this skill whenever the user mentions packet drops, rx_dropped, softnet, ring buffers, NIC drops, bond imbalance, TAP interfaces, OVS drops, OVN datapath issues, or network congestion.
allowed-tools: read, grep, bash
---

# Network Traffic Drops Troubleshooting

Use this runbook to diagnose packet drops in gateway, compute, virtualization, OVS, or OVN network paths. Keep commands read-only until the user explicitly approves a mitigation.

## Entry Decision Tree

```text
User symptom -> what is dropping?
  - rx_dropped increasing on bond/NIC -> Phase 1: Interface diagnosis
  - softnet drops or squeezes high -> Phase 2: Softnet analysis
  - specific VM/container suspected -> Phase 3: Source correlation
  - OVS/OVN datapath involved -> Phase 4: OVS/OVN correlation
  - unknown/general -> Phase 1 -> Phase 2 -> Phase 3
```

## Phase 1: Interface Diagnosis

Goal: identify drop type and affected interfaces.

```bash
ip -s link show <interface>
ethtool -S <nic> | grep -Ei "drop|miss|buffer|fifo|error"
ethtool -g <nic>
ethtool -k <nic> | grep -Ei "gro|lro|tso|gso|rx|tx"
```

Drop classification:

| Counter | Likely meaning | Next action |
|---|---|---|
| `rx_dropped` on interface | Kernel path dropped packets | Check softnet and CPU budget. |
| NIC-specific `rx_dropped` | Hardware or driver path dropped packets | Check ring buffers and driver counters. |
| `rx_missed_errors` | FIFO overflow or receive pressure | Check RX ring size and interrupt distribution. |
| `rx_fifo_errors` | Ring or FIFO pressure | Check ring size, offloads, and packet rate. |

Decision:

- If NIC hardware counters are increasing, inspect the hardware/driver path first.
- If interface drops increase but NIC counters do not, inspect softnet and CPU processing.

## Phase 2: Softnet Analysis

Goal: determine whether packet processing is CPU-budget constrained.

```bash
cat /proc/net/softnet_stat
nproc
mpstat -P ALL 1 5
```

For `/proc/net/softnet_stat`, the second column is commonly used to identify `time_squeeze` pressure. Growing values suggest the CPU could not process all packets within budget.

Decision:

- Growing `time_squeeze` plus high CPU means CPU/network processing pressure.
- Drops with low CPU may point back to ring buffers, interrupt affinity, offloads, or driver behavior.

## Phase 3: Source Correlation

Goal: identify whether a VM, container, TAP, or workload is causing bursts.

```bash
for tap in /sys/class/net/tap*; do
  name=$(basename "$tap")
  rx=$(cat "$tap/statistics/rx_bytes" 2>/dev/null || echo 0)
  tx=$(cat "$tap/statistics/tx_bytes" 2>/dev/null || echo 0)
  printf "%s rx=%s tx=%s\n" "$name" "$rx" "$tx"
done | sort -k2 -nr | head
```

Adapt the mapping command to the local platform:

```bash
incus list -c nsN4
virsh domiflist <domain>
ip link show
```

Correlation pattern:

1. Identify the timestamp when gateway or NIC drops increased.
2. Find TAP/container/VM interfaces with traffic spikes in the same window.
3. Map interface to workload.
4. Classify the source as expected burst, misconfiguration, abuse, or capacity issue.

## Phase 4: OVS/OVN Correlation

Goal: identify whether virtual switch or OVN datapath behavior is involved.

```bash
ovs-appctl dpctl/show
ovs-appctl dpctl/dump-flows | wc -l
ovs-vsctl list interface | grep -Ei "name|error|statistics"
ovs-appctl coverage/show
```

If OVN is present, inspect control-plane health separately before changing datapath settings:

```bash
ovn-nbctl show
ovn-sbctl show
ovn-sbctl list chassis
```

Decision:

- High datapath misses may indicate flow churn or cache pressure.
- OVS interface errors require interface-level investigation.
- OVN topology or route issues should be handled as an OVN troubleshooting task, not as a NIC tuning task.

## Phase 5: Mitigation Ranking

Do not apply mitigations without explicit user approval.

Low risk:

- Increase RX/TX ring buffer within documented NIC maximums.
- Adjust a single clearly implicated offload when evidence supports it.
- Rate-limit or move a clearly identified noisy workload.

Medium risk:

- RSS or IRQ affinity tuning.
- Flow steering changes.
- Planned workload evacuation.

High risk:

- Maximum ring sizes without latency analysis.
- Major OVS/OVN reconfiguration.
- Driver, firmware, or kernel changes.

Avoid jumping directly to very large ring buffers. They may hide drops while increasing latency and buffering.

## Phase 6: Verification

After an approved mitigation:

```bash
ip -s link show <interface>
cat /proc/net/softnet_stat
ethtool -S <nic> | grep -Ei "drop|miss|buffer|fifo|error"
```

Compare counters before and after the change over a defined interval. Report whether the same counters stopped increasing, slowed down, or continued unchanged.

## Output Format

Return:

1. Symptom summary.
2. Evidence gathered, with commands and relevant counters.
3. Probable root cause and confidence.
4. Correlated source workload, if identified.
5. Ranked mitigations with risk level.
6. Verification plan.
7. Any commands that require explicit approval before execution.
