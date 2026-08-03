// Implementacao inerte da superficie Enterprise, usada pela edicao Community.
//
// Este arquivo e AGPL, como o resto de `web/src/` fora de `web/src/ee/`. Ele
// existe para que o painel continue compilando e rodando quando `web/src/ee/`
// nao esta presente (LUC-19). O `vite.config.ts` aponta o alias `@ee` para ca
// quando a pasta EE nao existe ou `VITE_QUARK_EE` nao esta ligado, e o
// `tsconfig` aponta para ca sempre, o que faz deste arquivo o contrato de tipo
// que a implementacao real precisa satisfazer.
//
// Nenhum destes componentes chega a renderizar na edicao Community: o servidor
// nao roda em modo cloud, entao `me.multi_tenant` e falso e as telas que
// dependem disso nunca sao alcancadas.
import type { RouteObject } from "react-router";
import type { MeResponse } from "@/lib/types";

/** Ligada so na edicao Enterprise. */
export const eeEnabled = false;

/** Rotas Enterprise montadas pelo router. Vazio aqui. */
export const eeRoutes: RouteObject[] = [];

/** Rota publica de aceite de convite, fora da arvore autenticada. */
export const eePublicRoutes: RouteObject[] = [];

/** Onboarding de workspace. Inalcancavel sem modo cloud. */
export function WorkspaceGate(_props: { me: MeResponse }) {
  return null;
}

/** Seletor de workspace no topo da sidebar. */
export function WorkspaceSwitcher() {
  return null;
}
