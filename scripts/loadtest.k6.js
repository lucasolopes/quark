// Teste de carga do quark com k6 — foca no caminho quente do redirect (GET /:code).
//
// Rode NA VPS (RTT ~0) pra medir capacidade real do servidor, não a rede:
//   docker run --rm -i --network host \
//     -e QUARK_URL=https://quark.meuchat.ai \
//     -v "$PWD/scripts/loadtest.k6.js:/loadtest.js" \
//     grafana/k6 run /loadtest.js
//
// Parâmetros por env (opcionais):
//   QUARK_URL   base do serviço (default http://localhost:8080)
//   VUS         VUs no platô (default 200)
//   DURATION    duração do platô (default 30s)
import http from 'k6/http';
import { check } from 'k6';

const BASE = (__ENV.QUARK_URL || 'http://localhost:8080').replace(/\/$/, '');
const VUS = parseInt(__ENV.VUS || '200', 10);
const DURATION = __ENV.DURATION || '30s';

export const options = {
  scenarios: {
    redirect: {
      executor: 'ramping-vus',
      startVUs: 0,
      stages: [
        { duration: '10s', target: VUS }, // sobe
        { duration: DURATION, target: VUS }, // platô
        { duration: '5s', target: 0 }, // desce
      ],
      gracefulRampDown: '5s',
    },
  },
  thresholds: {
    http_req_failed: ['rate<0.01'], // <1% de erro — vale em qualquer lugar
    // A latência depende da DISTÂNCIA do cliente até a VPS (RTT), não do quark.
    // Default generoso pra não falhar por geografia; ajuste com -e P95_MS=<ms>.
    http_req_duration: [`p(95)<${__ENV.P95_MS || '800'}`],
  },
};

// Cria um link uma vez; os VUs martelam o redirect dele.
export function setup() {
  const res = http.post(`${BASE}/`, JSON.stringify({ url: 'https://example.com/loadtest' }), {
    headers: { 'Content-Type': 'application/json' },
  });
  const code = res.json('code');
  if (!code) {
    throw new Error(`setup falhou ao criar link: HTTP ${res.status} — ${res.body}`);
  }
  return { code };
}

export default function (data) {
  // redirects: 0 -> NÃO segue o 302; medimos o quark, não o destino.
  const res = http.get(`${BASE}/${data.code}`, { redirects: 0 });
  check(res, { 'status é 302': (r) => r.status === 302 });
}
