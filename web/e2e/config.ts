// Shared constants for the E2E harness. These must match docker-compose.e2e.yml
// (the Keycloak realm) and the quark env that global-setup starts.
export const PANEL = "http://localhost:5173";
export const API = "http://localhost:8080";
export const KEYCLOAK = "http://localhost:8081";
export const REALM = "quark";
export const DISCOVERY = `${KEYCLOAK}/realms/${REALM}/.well-known/openid-configuration`;

export const ADMIN_TOKEN = "dev-admin-token";

// Seeded Keycloak users (see e2e/keycloak/quark-realm.json).
export const ADMIN_USER = { username: "admin@quark.test", password: "password" };
export const READER_USER = { username: "reader@quark.test", password: "password" };
