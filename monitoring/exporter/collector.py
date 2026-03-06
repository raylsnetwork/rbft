#!/usr/bin/env python3
import os
import time
import json
import sqlite3
from dataclasses import dataclass
from pathlib import Path
from typing import List, Optional, Set, Tuple

import requests
from prometheus_client import start_http_server, Gauge

# --------------------------
# Env config
# --------------------------
RPC_URL         = os.environ.get("RPC_URL", "http://localhost:8545")
INSTANCE_LABEL  = os.environ.get("INSTANCE", "v0")  # e.g. v0, v1, v2, v3
GENESIS_PATH    = os.environ.get("GENESIS_PATH")
SCRAPE_INTERVAL = float(os.environ.get("SCRAPE_INTERVAL", "2"))
BALANCE_EVERY_N = int(os.environ.get("BALANCE_EVERY_N", "2"))
EXPORTER_PORT   = int(os.environ.get("EXPORTER_PORT", "9102"))
DB_PATH         = os.environ.get("DB_PATH", "/data/state.sqlite3")

@dataclass
class ValidatorConfig:
    label: str
    rpc_url: str
    db_path: str


@dataclass
class ValidatorState:
    config: ValidatorConfig
    conn: sqlite3.Connection
    loops: int = 0
    last_balance_eth: float = 0.0


def derive_db_path(base_path: str, label: str) -> str:
    """Derive a per-validator SQLite path so state is isolated."""
    if "{instance}" in base_path:
        return base_path.replace("{instance}", label)

    base_path = base_path.strip()
    path = Path(base_path)

    # If the base looks like an existing directory or explicitly ends with a separator,
    # store the SQLite file inside that directory.
    if base_path.endswith(os.path.sep) or path.exists() and path.is_dir():
        return str(path / f"state_{label}.sqlite3")

    if path.suffix:
        return str(path.with_name(f"{path.stem}_{label}{path.suffix}"))

    # Treat as directory-like path (even if it doesn't exist yet).
    return str(path / f"state_{label}.sqlite3")


def load_validator_configs() -> List[ValidatorConfig]:
    """Build validator configuration list from environment variables."""
    raw = os.environ.get("RPC_URLS")
    if not raw:
        return [
            ValidatorConfig(
                label=INSTANCE_LABEL,
                rpc_url=RPC_URL,
                db_path=derive_db_path(DB_PATH, INSTANCE_LABEL),
            )
        ]

    configs: List[ValidatorConfig] = []
    entries = [item.strip() for item in raw.split(",") if item.strip()]
    for idx, entry in enumerate(entries):
        if "=" in entry:
            label, url = entry.split("=", 1)
            label = label.strip()
            url = url.strip()
        else:
            label = f"v{idx}"
            url = entry

        if not label:
            label = f"validator_{idx}"
        if not url:
            continue
        configs.append(
            ValidatorConfig(
                label=label,
                rpc_url=url,
                db_path=derive_db_path(DB_PATH, label),
            )
        )

    if not configs:
        # Fallback to single RPC_URL to avoid empty configuration
        configs.append(
            ValidatorConfig(
                label=INSTANCE_LABEL,
                rpc_url=RPC_URL,
                db_path=derive_db_path(DB_PATH, INSTANCE_LABEL),
            )
        )

    return configs

# --------------------------
# Prometheus metrics
# --------------------------
g_accounts = Gauge("rbft_state_accounts", "Number of discovered accounts", ["instance"])
g_balance = Gauge(
    "rbft_state_balance_eth",
    "Total balance (ETH) of discovered accounts",
    ["instance"],
)
g_txs = Gauge(
    "rbft_state_transactions",
    "Total transactions observed on-chain (sum of txs per block)",
    ["instance"],
)
g_height   = Gauge("rbft_block_height", "Latest block height observed", ["instance"])
g_pending_txs = Gauge("rbft_pending_txs", "Number of pending transactions in mempool", ["instance"])

# --------------------------
# JSON-RPC helpers
# --------------------------
def rpc(cfg: ValidatorConfig, method: str, params=None):
    if params is None:
        params = []
    try:
        print(f"[{cfg.label}] RPC DEBUG: Calling {method} with params {params}")
        r = requests.post(
            cfg.rpc_url,
            headers={"Content-Type": "application/json"},
            data=json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}),
            timeout=5,
        )
        r.raise_for_status()
        j = r.json()
        if "error" in j:
            raise RuntimeError(j["error"])
        print(f"[{cfg.label}] RPC DEBUG: {method} success")
        return j["result"]
    except Exception as e:
        print(f"[{cfg.label}] RPC ERROR: {method}: {e}")
        raise RuntimeError(f"RPC error on {method}: {e}")

def hex_to_int(h: str) -> int:
    return int(h, 16)

def int_to_hex(n: int) -> str:
    return hex(n)

# --------------------------
# Storage
# --------------------------
def ensure_db(label: str, conn: sqlite3.Connection):
    print(f"[{label}] DB DEBUG: Ensuring database schema exists")
    conn.execute("""
    CREATE TABLE IF NOT EXISTS meta (
      key TEXT PRIMARY KEY,
      value TEXT
    )""")
    conn.execute("""
    CREATE TABLE IF NOT EXISTS accounts (
      address TEXT PRIMARY KEY
    )""")
    conn.execute("""
    CREATE TABLE IF NOT EXISTS chain (
      last_block INTEGER DEFAULT 0,
      total_txs  INTEGER DEFAULT 0
    )""")
    cur = conn.execute("SELECT COUNT(*) FROM chain")
    if cur.fetchone()[0] == 0:
        conn.execute("INSERT INTO chain (last_block,total_txs) VALUES (0,0)")
        print(f"[{label}] DB DEBUG: Initialized chain table with (0,0)")
    conn.commit()
    print(f"[{label}] DB DEBUG: Database schema ensured")

def db_get_last_block_and_txs(label: str, conn) -> Tuple[int,int]:
    cur = conn.execute("SELECT last_block,total_txs FROM chain LIMIT 1")
    row = cur.fetchone()
    print(f"[{label}] DB DEBUG: Retrieved last_block={row[0]}, total_txs={row[1]}")
    return (row[0], row[1])

def db_set_last_block_and_txs(label: str, conn, last_block: int, total_txs: int):
    print(f"[{label}] DB DEBUG: Setting last_block={last_block}, total_txs={total_txs}")
    conn.execute("UPDATE chain SET last_block=?, total_txs=?", (last_block, total_txs))
    conn.commit()

def db_add_accounts(label: str, conn, addrs: Set[str]):
    if not addrs:
        print(f"[{label}] DB DEBUG: No addresses to add")
        return
    print(f"[{label}] DB DEBUG: Adding {len(addrs)} addresses to DB: {addrs}")
    conn.executemany(
        "INSERT OR IGNORE INTO accounts(address) VALUES (?)",
        [(a.lower(),) for a in addrs],
    )
    conn.commit()
    print(f"[{label}] DB DEBUG: Successfully added addresses")

def db_list_accounts(label: str, conn) -> Set[str]:
    cur = conn.execute("SELECT address FROM accounts")
    accounts = {r[0] for r in cur.fetchall()}
    print(f"[{label}] DB DEBUG: Retrieved {len(accounts)} accounts from DB: {accounts}")
    return accounts

def db_count_accounts(label: str, conn) -> int:
    cur = conn.execute("SELECT COUNT(*) FROM accounts")
    count = cur.fetchone()[0]
    print(f"[{label}] DB DEBUG: Account count = {count}")
    return count

def db_reset_if_chain_ahead(label: str, conn, current_block: int):
    """Reset database if stored last_block is significantly ahead of current chain"""
    last_block, _ = db_get_last_block_and_txs(label, conn)
    
    # If stored block is more than 100 blocks ahead of current chain, reset
    if last_block > current_block + 100:
        print(
            f"[{label}] DB DEBUG: Chain reset detected "
            f"(stored: {last_block}, current: {current_block}). Resetting database."
        )
        conn.execute("UPDATE chain SET last_block=0, total_txs=0")
        conn.execute("DELETE FROM accounts")
        conn.execute("DELETE FROM meta")
        conn.commit()
        print(f"[{label}] DB DEBUG: Database reset complete")
        return True
    return False

# --------------------------
# Seeding from genesis alloc (optional)
# --------------------------
def seed_from_genesis(label: str, conn):
    if not GENESIS_PATH:
        print(f"[{label}] GENESIS DEBUG: No genesis path provided")
        return
    try:
        print(f"[{label}] GENESIS DEBUG: Loading genesis from {GENESIS_PATH}")
        with open(GENESIS_PATH, "r") as f:
            g = json.load(f)
        alloc = g.get("alloc") or g.get("genesis", {}).get("alloc") or {}
        addrs = set()
        for a in alloc.keys():
            if a.startswith("0x") and len(a) == 42:
                addrs.add(a.lower())
        if addrs:
            print(f"[{label}] GENESIS DEBUG: Found {len(addrs)} addresses in genesis")
            db_add_accounts(label, conn, addrs)
            print(f"[{label}] Seeded {len(addrs)} accounts from genesis")
        else:
            print(f"[{label}] GENESIS DEBUG: No addresses found in genesis alloc")
    except Exception as e:
        print(f"[{label}] Failed to seed from genesis: {e}")

# --------------------------
# Chain scan / discovery
# --------------------------
def latest_block_number(cfg: ValidatorConfig) -> int:
    print(f"[{cfg.label}] BLOCK DEBUG: Getting latest block number")
    result = hex_to_int(rpc(cfg, "eth_blockNumber"))
    print(f"[{cfg.label}] BLOCK DEBUG: Latest block number = {result}")
    return result

def get_block(cfg: ValidatorConfig, num: int) -> Optional[dict]:
    h = int_to_hex(num)
    print(f"[{cfg.label}] BLOCK DEBUG: Fetching block {num} (hex: {h})")
    result = rpc(cfg, "eth_getBlockByNumber", [h, True])
    if result:
        txs = len(result.get("transactions", []))
        print(
            f"[{cfg.label}] BLOCK DEBUG: Successfully retrieved block {num} "
            f"with {txs} transactions"
        )
    else:
        print(f"[{cfg.label}] BLOCK DEBUG: Block {num} not found")
    return result

def discover_accounts_from_block(cfg: ValidatorConfig, block: dict) -> Tuple[int, Set[str]]:
    addrs = set()
    txs = block.get("transactions") or []

    block_num = hex_to_int(block.get("number", "0x0"))
    print(f"[{cfg.label}] DISCOVERY DEBUG: Processing block {block_num}, {len(txs)} transactions")

    tx_count = len(txs)

    # If transactions are full objects
    if tx_count and isinstance(txs[0], dict):
        print(f"[{cfg.label}] DISCOVERY DEBUG: Transactions are full objects, processing directly")
        for i, tx in enumerate(txs):
            frm = tx.get("from")
            to = tx.get("to")
            print(
                f"[{cfg.label}] DISCOVERY DEBUG: Tx {i}: from='{frm}' "
                f"(type: {type(frm)}), to='{to}' (type: {type(to)})"
            )
            
            if frm: 
                frm_lower = frm.lower()
                addrs.add(frm_lower)
                print(f"[{cfg.label}] DISCOVERY DEBUG:   Added from address: '{frm_lower}'")
            else:
                print(f"[{cfg.label}] DISCOVERY DEBUG:   No 'from' address in tx")
                
            if to: 
                to_lower = to.lower()
                addrs.add(to_lower)
                print(f"[{cfg.label}] DISCOVERY DEBUG:   Added to address: '{to_lower}'")
            else:
                print(
                    f"[{cfg.label}] DISCOVERY DEBUG:   No 'to' address "
                    f"(possible contract creation)"
                )
    else:
        print(f"[{cfg.label}] DISCOVERY DEBUG: Tx list appears to be hashes, fetching individually")
        full_txs = []
        for txhash in txs:
            try:
                print(f"[{cfg.label}] DISCOVERY DEBUG: Fetching tx {txhash[:10]}...")
                txobj = rpc(cfg, "eth_getTransactionByHash", [txhash])
                if txobj:
                    full_txs.append(txobj)
                    frm = txobj.get("from")
                    to = txobj.get("to")
                    print(
                        f"[{cfg.label}] DISCOVERY DEBUG: Fetched tx {txhash[:10]}: "
                        f"from='{frm}', to='{to}'"
                    )
                    if frm: 
                        frm_lower = frm.lower()
                        addrs.add(frm_lower)
                        print(f"[{cfg.label}] DISCOVERY DEBUG:   Added from address: '{frm_lower}'")
                    if to: 
                        to_lower = to.lower()
                        addrs.add(to_lower)
                        print(f"[{cfg.label}] DISCOVERY DEBUG:   Added to address: '{to_lower}'")
            except Exception as e:
                print(f"[{cfg.label}] DISCOVERY WARN: fetch tx {txhash[:10]} err: {e}")
        tx_count = len(full_txs)

    print(
        f"[{cfg.label}] DISCOVERY DEBUG: Block {block_num} summary: "
        f"{tx_count} transactions, {len(addrs)} unique addresses: {addrs}"
    )
    return (tx_count, addrs)

def get_balance(cfg: ValidatorConfig, addr: str) -> float:
    print(f"[{cfg.label}] BALANCE DEBUG: Getting balance for {addr}")
    w = rpc(cfg, "eth_getBalance", [addr, "latest"])
    balance_wei = hex_to_int(w)
    
    # Convert wei to ETH properly
    balance_eth = balance_wei / 10**18
    
    print(
        f"[{cfg.label}] BALANCE DEBUG: Balance for {addr} = {balance_wei} wei "
        f"= {balance_eth:.6f} ETH"
    )
    return balance_eth

def get_pending_txs(cfg: ValidatorConfig) -> int:
    """Returns the count of pending transactions in the mempool."""
    try:
        print(f"[{cfg.label}] PENDING DEBUG: Getting pending transactions")
        res = rpc(cfg, "eth_getBlockByNumber", ["pending", True])
        if not res:
            print(f"[{cfg.label}] PENDING DEBUG: No pending block found")
            return 0
        txs = res.get("transactions", [])
        print(f"[{cfg.label}] PENDING DEBUG: Found {len(txs)} pending transactions")
        return len(txs)
    except Exception as e:
        print(f"[{cfg.label}] PENDING ERROR: pending txs error: {e}")
        return 0


def initialize_validator(cfg: ValidatorConfig) -> ValidatorState:
    print(f"[{cfg.label}] ========== INITIALIZING VALIDATOR ==========")
    print(f"[{cfg.label}] Configuration:")
    print(f"[{cfg.label}]   RPC_URL: {cfg.rpc_url}")
    print(f"[{cfg.label}]   DB_PATH: {cfg.db_path}")
    print(f"[{cfg.label}]   GENESIS_PATH: {GENESIS_PATH}")

    db_path = Path(cfg.db_path)
    db_path.parent.mkdir(parents=True, exist_ok=True)

    print(f"[{cfg.label}] DB DEBUG: Connecting to database at {cfg.db_path}")
    conn = sqlite3.connect(cfg.db_path, isolation_level=None, check_same_thread=False)
    ensure_db(cfg.label, conn)

    try:
        current_head = latest_block_number(cfg)
        print(f"[{cfg.label}] INITIAL DEBUG: Current chain head: {current_head}")
        db_reset_if_chain_ahead(cfg.label, conn, current_head)
    except Exception as e:
        print(f"[{cfg.label}] INITIAL WARN: Could not get initial block number: {e}")

    seed_from_genesis(cfg.label, conn)

    initial_accounts = db_count_accounts(cfg.label, conn)
    last_block, total_txs = db_get_last_block_and_txs(cfg.label, conn)
    print(
        f"[{cfg.label}] INITIAL STATE: accounts={initial_accounts}, "
        f"last_block={last_block}, total_txs={total_txs}"
    )

    return ValidatorState(config=cfg, conn=conn)


def run_validator_cycle(state: ValidatorState):
    cfg = state.config
    label = cfg.label
    conn = state.conn

    print(f"[{label}] ========== LOOP {state.loops} ==========")
    print(f"[{label}] polling RPC {cfg.rpc_url}")
    try:
        head = latest_block_number(cfg)
        print(f"[{label}] MAIN DEBUG: Current head block: {head}")
    except Exception as e:
        print(f"[{label}] MAIN ERROR: rpc head error: {e}")
        state.loops += 1
        return

    last_block, total_txs = db_get_last_block_and_txs(label, conn)
    print(f"[{label}] MAIN DEBUG: DB state - last_block: {last_block}, total_txs: {total_txs}")

    if db_reset_if_chain_ahead(label, conn, head):
        last_block, total_txs = db_get_last_block_and_txs(label, conn)
        print(
            f"[{label}] MAIN DEBUG: Chain reset detected, reloaded DB state: "
            f"last_block={last_block}, total_txs={total_txs}"
        )

    if last_block == 0 and head > 0:
        last_block = head - 1
        print(f"[{label}] MAIN DEBUG: Reset last_block to: {last_block}")

    updated_total_txs = total_txs

    if head > last_block:
        blocks_to_scan = head - last_block
        print(
            f"[{label}] MAIN DEBUG: Need to scan {blocks_to_scan} blocks "
            f"({last_block + 1} to {head})"
        )
        discovered: Set[str] = set()
        total_new_txs = 0

        for n in range(last_block + 1, head + 1):
            try:
                print(f"[{label}] MAIN DEBUG: Scanning block {n}")
                blk = get_block(cfg, n)
                if not blk:
                    print(f"[{label}] MAIN DEBUG: Block {n} not found, skipping")
                    continue

                tx_count, addrs = discover_accounts_from_block(cfg, blk)
                total_new_txs += tx_count
                discovered |= addrs

                print(
                    f"[{label}] MAIN DEBUG: Block {n} results: {tx_count} txs, "
                    f"{len(addrs)} new addresses"
                )
                print(
                    f"[{label}] MAIN DEBUG: Cumulative: {total_new_txs} total new txs, "
                    f"{len(discovered)} total discovered addresses"
                )
            except Exception as e:
                print(f"[{label}] MAIN ERROR: scan block {n} error: {e}")
                break

        updated_total_txs += total_new_txs
        print(
            f"[{label}] MAIN DEBUG: Scan complete: {len(discovered)} addresses discovered, "
            f"{total_new_txs} new transactions"
        )
        print(f"[{label}] MAIN DEBUG: Addresses discovered: {discovered}")

        if discovered:
            print(f"[{label}] MAIN DEBUG: Adding discovered addresses to DB")
            db_add_accounts(label, conn, discovered)
        else:
            print(f"[{label}] MAIN DEBUG: No new addresses to add")

        print(
            f"[{label}] MAIN DEBUG: Updating DB with last_block={head}, "
            f"total_txs={updated_total_txs}"
        )
        db_set_last_block_and_txs(label, conn, head, updated_total_txs)
        print(f"[{label}] MAIN DEBUG: DB update complete")
    else:
        print(f"[{label}] MAIN DEBUG: No new blocks to scan (head={head}, last_block={last_block})")

    # Update metrics
    try:
        current_accounts = db_count_accounts(label, conn)
        print(
            f"[{label}] METRICS DEBUG: Setting metrics - height={head}, "
            f"txs={updated_total_txs}, accounts={current_accounts}"
        )
        g_height.labels(label).set(head)
        g_txs.labels(label).set(updated_total_txs)
        g_accounts.labels(label).set(current_accounts)
        print(f"[{label}] METRICS DEBUG: Metrics updated successfully")
    except Exception as e:
        print(f"[{label}] METRICS ERROR: metric set error: {e}")

    # Pending transactions tracking
    try:
        print(f"[{label}] PENDING DEBUG: Checking pending transactions")
        pending = get_pending_txs(cfg)
        g_pending_txs.labels(label).set(pending)
        print(f"[{label}] PENDING DEBUG: Pending transactions count: {pending}")
    except Exception as e:
        print(f"[{label}] PENDING ERROR: pending tx update error: {e}")

    # Balance updates (every N loops)
    total_eth = state.last_balance_eth
    recompute_balance = state.loops % BALANCE_EVERY_N == 0
    if recompute_balance:
        print(f"[{label}] BALANCE DEBUG: Performing balance update (loop {state.loops})")
        addrs = list(db_list_accounts(label, conn))
        total_eth = 0.0
        print(f"[{label}] BALANCE DEBUG: Calculating total balance for {len(addrs)} accounts")

        for a in addrs:
            try:
                balance_eth = get_balance(cfg, a)
                total_eth += balance_eth
                print(f"[{label}] BALANCE DEBUG: Account {a}: {balance_eth:.6f} ETH")
            except Exception as e:
                print(f"[{label}] BALANCE ERROR: Failed to get balance for {a}: {e}")

        state.last_balance_eth = total_eth

    g_balance.labels(label).set(total_eth)
    print(f"[{label}] BALANCE DEBUG: Total balance: {total_eth:.6f} ETH")

    # Final status output
    current_account_count = db_count_accounts(label, conn)
    print(f"[{label}] ========== STATUS ==========")
    print(f"[{label}] height={head}, txs={updated_total_txs}, accounts={current_account_count}")
    print(f"[{label}] ============================")

    state.loops += 1

# --------------------------
# Main loop
# --------------------------
def main():
    configs = load_validator_configs()
    print("[collector] ========== STARTING COLLECTOR ==========")
    print(f"[collector] Managing {len(configs)} validator(s)")
    for cfg in configs:
        print(f"[collector]   - {cfg.label}: {cfg.rpc_url} (db: {cfg.db_path})")

    start_http_server(EXPORTER_PORT)
    print(f"[collector] Exporter listening on :{EXPORTER_PORT}")

    states = [initialize_validator(cfg) for cfg in configs]

    while True:
        for state in states:
            run_validator_cycle(state)
        print(f"[collector] Sleeping for {SCRAPE_INTERVAL} seconds...")
        time.sleep(SCRAPE_INTERVAL)

if __name__ == "__main__":
    main()
