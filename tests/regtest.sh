#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
COMPOSE="docker compose -p rgb-lib-wasm -f $SCRIPT_DIR/docker-compose.ci.yml"
SERVICES="bitcoind esplora proxy proxy-b proxy-auth regtest-helper vss-postgres vss-server vss-cors-proxy"

case "$1" in
    start)
        echo "Starting services: $SERVICES"
        $COMPOSE up -d $SERVICES

        echo "Waiting for regtest-helper..."
        for i in $(seq 1 60); do
            if curl -sf http://127.0.0.1:8080/status > /dev/null 2>&1; then
                break
            fi
            sleep 1
        done

        BCLI="$COMPOSE exec -T bitcoind bitcoin-cli -regtest -rpcuser=user -rpcpassword=default_password"
        BCLI_ESPLORA="$COMPOSE exec -T esplora cli"

        echo "Waiting for esplora to start..."
        for i in $(seq 1 120); do
            if $COMPOSE logs esplora 2>&1 | grep -q 'run: nginx:'; then
                break
            fi
            if [ $i -eq 120 ]; then
                echo "Timeout waiting for esplora to start"
                exit 1
            fi
            sleep 1
        done

        echo "Connecting bitcoind nodes as peers..."
        $BCLI addnode "esplora:18444" "add" 2>/dev/null || true
        $BCLI_ESPLORA addnode "bitcoind:18444" "add" 2>/dev/null || true
        sleep 2

        echo "Mining initial 111 blocks..."
        curl -s -X POST http://127.0.0.1:8080/mine \
            -H 'Content-Type: application/json' \
            -d '{"blocks": 111}'
        echo ""

        echo "Waiting for esplora to sync..."
        for i in $(seq 1 120); do
            HEIGHT=$(curl -sf http://127.0.0.1:8094/regtest/api/blocks/tip/height 2>/dev/null || echo "0")
            if [ "$HEIGHT" -ge 111 ] 2>/dev/null; then
                break
            fi
            sleep 1
        done

        echo "Waiting for VSS server (via CORS proxy on :8082)..."
        for i in $(seq 1 60); do
            if curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8082/vss/getObject 2>/dev/null | grep -q "400\|401\|405\|200"; then
                echo "VSS server ready"
                break
            fi
            sleep 1
        done

        echo "Ready. Services running on:"
        echo "  bitcoind RPC:    http://127.0.0.1:18443"
        echo "  esplora:         http://127.0.0.1:8094/regtest/api"
        echo "  RGB proxy:       http://127.0.0.1:3000/json-rpc"
        echo "  RGB proxy B:     http://127.0.0.1:3010/json-rpc"
        echo "  RGB proxy auth:  http://127.0.0.1:3020/json-rpc"
        echo "  regtest-helper:  http://127.0.0.1:8080"
        echo "  VSS server:      http://127.0.0.1:8081 (direct), http://127.0.0.1:8082 (CORS proxy)"
        ;;
    stop)
        echo "Stopping services..."
        $COMPOSE down
        ;;
    mine)
        BLOCKS="${2:-1}"
        curl -s -X POST http://127.0.0.1:8080/mine \
            -H 'Content-Type: application/json' \
            -d "{\"blocks\": $BLOCKS}"
        echo ""
        ;;
    fund)
        if [ -z "$2" ]; then
            echo "Usage: $0 fund <address> [amount]"
            exit 1
        fi
        AMOUNT="${3:-1.0}"
        curl -s -X POST http://127.0.0.1:8080/fund \
            -H 'Content-Type: application/json' \
            -d "{\"address\": \"$2\", \"amount\": \"$AMOUNT\"}"
        echo ""
        ;;
    status)
        echo "regtest-helper: $(curl -sf http://127.0.0.1:8080/status 2>/dev/null || echo 'not reachable')"
        echo "esplora:        $(curl -sf http://127.0.0.1:8094/regtest/api/blocks/tip/height 2>/dev/null || echo 'not reachable')"
        echo "block height:   $(curl -sf http://127.0.0.1:8080/height 2>/dev/null || echo 'unknown')"
        ;;
    *)
        echo "Usage: $0 {start|stop|mine [N]|fund <address> [amount]|status}"
        ;;
esac
