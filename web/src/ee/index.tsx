// quark Enterprise Edition, painel.
//
// ATENCAO: este diretorio NAO e AGPL. Ele e coberto pela quark Enterprise
// Edition License em `web/src/ee/LICENSE`. Todo o resto de `web/` e
// AGPL-3.0-only. Ver `docs/LICENSING.md`.
//
// Barrel que o core importa pelo alias `@ee`. O `vite.config.ts` resolve esse
// alias para ca quando `VITE_QUARK_EE` esta ligado e a pasta existe, e para
// `@/lib/ee-stub` caso contrario. As duas implementacoes tem a mesma forma, o
// que e checado pelo `satisfies` no fim deste arquivo.
import { lazy } from "react";
import type { RouteObject } from "react-router";

import { suspended } from "@/app/suspended";

export { WorkspaceGate } from "./WorkspaceGate";
export { WorkspaceSwitcher } from "./WorkspaceSwitcher";

const AcceptInvite = lazy(() => import("./AcceptInvite").then((m) => ({ default: m.AcceptInvite })));
const Members = lazy(() => import("./Members").then((m) => ({ default: m.Members })));
const SsoDomains = lazy(() => import("./SsoDomains").then((m) => ({ default: m.SsoDomains })));
const OidcProvider = lazy(() => import("./OidcProvider").then((m) => ({ default: m.OidcProvider })));
const Domains = lazy(() => import("./Domains").then((m) => ({ default: m.Domains })));

export const eeEnabled = true;

/** Telas Enterprise dentro da arvore autenticada, montadas pelo `router.tsx`. */
export const eeRoutes: RouteObject[] = [
  { path: "members", element: suspended(<Members />) },
  { path: "sso-domains", element: suspended(<SsoDomains />) },
  { path: "sso-provider", element: suspended(<OidcProvider />) },
  { path: "domains", element: suspended(<Domains />) },
];

/**
 * Aceite de convite. Fica fora da arvore autenticada de proposito: quem chega
 * por um link de convite ainda nao tem workspace, e montar isso sob a arvore
 * autenticada prenderia a pessoa no `WorkspaceGate`.
 */
export const eePublicRoutes: RouteObject[] = [
  { path: "/invite/:token", element: suspended(<AcceptInvite />) },
];
