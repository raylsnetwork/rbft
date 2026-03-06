# RBFT Kubernetes Runbook

This guide covers:

- starting a Kubernetes testnet
- verifying block production
- running megatx load
- verifying the stall monitor
- retrieving logs

All commands assume:

- `kubectl` context points at the correct cluster
- default namespace is `rbft` unless you set `RBFT_KUBE_NAMESPACE`

Set this once in your shell so the same docs work for any namespace:

```sh
NS="${RBFT_KUBE_NAMESPACE:-rbft}"
```

## 1) Start a fresh testnet

```sh
RBFT_KUBE=true RBFT_NUM_NODES=4 RBFT_MAX_ACTIVE_VALIDATORS=4 make testnet_start
```

To deploy into a different namespace:

```sh
RBFT_KUBE_NAMESPACE=rbft-alt \
  RBFT_KUBE=true RBFT_NUM_NODES=4 RBFT_MAX_ACTIVE_VALIDATORS=4 \
  make testnet_start
```

Notes:

- This builds and pushes the image, then deploys a StatefulSet.
- In kube mode, the namespace is created if it doesn't exist.
- You can Ctrl+C locally once pods are running; the chain stays up in Kubernetes.

Check pods:

```sh
kubectl get pods -n "$NS" -l app=rbft-node -o wide
```

## 2) Verify blocks are being produced

Quick check (compare two samples):

```sh
kubectl logs -n "$NS" rbft-node-0 --since=1m | grep latest_block | tail -n 1
sleep 10
kubectl logs -n "$NS" rbft-node-0 --since=1m | grep latest_block | tail -n 1
```

Heights should increase.

Live watch (Ctrl+C to stop):

```sh
kubectl logs -n "$NS" rbft-node-0 -f | grep latest_block
```

## 3) Deploy the stall monitor

The monitor checks height every 30s and triggers when it didn't see the height increase.
By default it captures immediately when a stall is detected (no extra grace wait).
It saves full node logs to the PVC, captures Kubernetes diagnostics for disconnects,
then scales the RBFT StatefulSet to 0. If the megatx deployment exists, it is
scaled to 0 as well.

### Slack notifications (optional)

Create a Secret with your Slack webhook URL:

```sh
kubectl create secret generic rbft-monitor-slack -n "$NS" \
  --from-literal=url='<WEBHOOK_URL>' \
  --dry-run=client -o yaml | kubectl apply -f -
```

Apply the monitor Job:

```sh
kubectl delete job rbft-monitor -n "$NS" --ignore-not-found
kubectl apply -n "$NS" -f scripts/kube/rbft-monitor.yaml
```

Check status:

```sh
kubectl get jobs -n "$NS" rbft-monitor
kubectl logs -n "$NS" -l job-name=rbft-monitor --tail=50
```

Adjust detection in `scripts/kube/rbft-monitor.yaml`:

- `CHECK_INTERVAL` (default `30`) controls how often the monitor checks height.
  If the height does not advance on a check, it immediately captures logs and
  scales down the pods.
- `MEGATX_DEPLOYMENT` (default `rbft-megatx`) to control which load generator
  is stopped on stall.

## 4) Run megatx continuously

Build and push the megatx image:

```sh
REGISTRY=${RBFT_REGISTRY:?Set RBFT_REGISTRY to your container registry}
docker build -f Dockerfile.megatx \
  -t ${REGISTRY}/rbft-megatx:latest .
docker push ${REGISTRY}/rbft-megatx:latest
```

Deploy:

```sh
kubectl apply -n "$NS" -f scripts/kube/rbft-megatx.yaml
```

Verify:

```sh
kubectl get pods -n "$NS" -l app=rbft-megatx
kubectl logs -n "$NS" -l app=rbft-megatx --tail=50
```

Adjust megatx settings in `scripts/kube/rbft-megatx.yaml`:

- `MEGATX_NUM_TXS` per batch
- `MEGATX_TARGET_TPS` (0 = unlimited)
- `MEGATX_SLEEP_SECONDS` between batches
- `MEGATX_MAX_WAIT_SECONDS` (default `30`) to stop waiting for missing txs
- `MEGATX_NUM_NODES` to match the number of rbft nodes in that namespace

Note: `imagePullPolicy: Always` is set so new `:latest` pushes are pulled.

## 4.1) Reset megatx after rbft pods were terminated

If the rbft nodes were scaled to 0, megatx will fail and may crashloop. To reset:

```sh
kubectl delete deployment -n "$NS" rbft-megatx --ignore-not-found
kubectl delete pod -n "$NS" -l app=rbft-megatx --ignore-not-found
```

Make sure rbft pods are back up:

```sh
kubectl get pods -n "$NS" -l app=rbft-node -o wide
```

Re-deploy megatx:

```sh
kubectl apply -n "$NS" -f scripts/kube/rbft-megatx.yaml
kubectl get pods -n "$NS" -l app=rbft-megatx -w
```

If megatx still complains about missing flags, rebuild/push the image and re-apply:

```sh
REGISTRY=${RBFT_REGISTRY:?Set RBFT_REGISTRY to your container registry}
docker build -f Dockerfile.megatx \
  -t ${REGISTRY}/rbft-megatx:latest .
docker push ${REGISTRY}/rbft-megatx:latest
kubectl rollout restart deployment -n "$NS" rbft-megatx
```

## 5) Confirm monitor captured logs after a stall

When the chain stalls, the monitor saves logs to the PVC:
`/data/logs/<timestamp>/` and scales `rbft-node` to 0.

Captured diagnostics include (from the same timestamped folder on the PVC):

- `events.txt` (namespace events)
- `<pod>.describe.txt` and `<pod>.yaml` (container last state and termination reasons)
- `pods_status.txt`, `pods.txt`, `pvc.txt`, `statefulset.yaml`
- `<pod>.log` from `/data/logs/node.log` if present

These files are what you use to diagnose container restarts after the run.
`<pod>.describe.txt` includes `Last State` and termination reasons, and
`events.txt` shows OOMKilled/eviction/scheduling errors at the time.

Check completion:

```sh
kubectl get jobs -n "$NS" rbft-monitor
```

## 6) Retrieve logs from the PVC

Use a temporary reader pod:

```sh
kubectl apply -n "$NS" -f - <<'EOF'
apiVersion: v1
kind: Pod
metadata:
  name: rbft-monitor-logs-reader
spec:
  restartPolicy: Never
  containers:
    - name: reader
      image: busybox
      command: ["sh", "-c", "sleep 3600"]
      volumeMounts:
        - name: logs
          mountPath: /data
  volumes:
    - name: logs
      persistentVolumeClaim:
        claimName: rbft-monitor-logs
EOF
```

Wait for Running, then copy logs:

```sh
kubectl wait --for=condition=Ready pod/rbft-monitor-logs-reader -n "$NS" --timeout=120s
kubectl cp -n "$NS" rbft-monitor-logs-reader:/data/logs ./rbft-monitor-logs
kubectl delete pod -n "$NS" rbft-monitor-logs-reader
```

## 7) Notes on log completeness

RBFT pods now write logs to `/data/logs/node.log` on their PVCs, so the monitor
can capture full history even if Kubernetes rotates stdout logs.

## 8) Parallel Instances (Optional)

If you want parallel instances, use a second namespace and apply the same
manifests with `RBFT_KUBE_NAMESPACE` set (for the testnet) and explicit `-n` on
`kubectl` commands. Keep the monitor and megatx in the same namespace as their
target RBFT nodes.

Example:

```sh
RBFT_KUBE_NAMESPACE=rbft-alt \
  RBFT_KUBE=true RBFT_NUM_NODES=4 RBFT_MAX_ACTIVE_VALIDATORS=4 \
  make testnet_start

NS="${RBFT_KUBE_NAMESPACE:-rbft}"
kubectl apply -n "$NS" -f scripts/kube/rbft-monitor.yaml
kubectl apply -n "$NS" -f scripts/kube/rbft-megatx.yaml
```
