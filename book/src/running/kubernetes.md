# Kubernetes Deployment

RBFT supports deploying a full testnet to Kubernetes using a StatefulSet.

## Quick start

```bash
RBFT_KUBE=true RBFT_NUM_NODES=4 RBFT_MAX_ACTIVE_VALIDATORS=4 \
  make testnet_start
```

This builds and pushes the Docker image, creates the namespace (default: `rbft`),
deploys a ConfigMap with genesis and keys, and creates a StatefulSet.

Press Ctrl+C once pods are running — the chain continues in Kubernetes.

## Custom namespace

```bash
RBFT_KUBE_NAMESPACE=rbft-staging \
  RBFT_KUBE=true RBFT_NUM_NODES=4 RBFT_MAX_ACTIVE_VALIDATORS=4 \
  make testnet_start
```

## Verifying

```bash
NS="${RBFT_KUBE_NAMESPACE:-rbft}"
kubectl get pods -n "$NS" -l app=rbft-node -o wide
```

Check block production:

```bash
kubectl logs -n "$NS" rbft-node-0 -f | grep latest_block
```

## Stall monitor

The monitor checks block height every 30 seconds and captures logs if a stall
is detected:

```bash
kubectl apply -n "$NS" -f scripts/kube/rbft-monitor.yaml
```

Optional Slack notifications:

```bash
kubectl create secret generic rbft-monitor-slack -n "$NS" \
  --from-literal=url='<WEBHOOK_URL>'
```

## Load testing (megatx)

```bash
kubectl apply -n "$NS" -f scripts/kube/rbft-megatx.yaml
```

Configure via environment variables in the manifest:

- `MEGATX_NUM_TXS` — transactions per batch
- `MEGATX_TARGET_TPS` — target transactions per second (0 = unlimited)
- `MEGATX_SLEEP_SECONDS` — pause between batches

## Retrieving logs from PVC

See the full Kubernetes runbook at `doc/rbft-kube-runbook.md` for detailed
instructions on log retrieval, parallel instances, and troubleshooting.
