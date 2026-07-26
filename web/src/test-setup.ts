import "@testing-library/jest-dom/vitest";
import { configure } from "@testing-library/react";

// The default findBy* timeout is 1s. Vitest runs the 48 test files in
// parallel, so a slow runner can push a render past that and fail a test that
// passes on its own. Raising the ceiling does not slow the suite: a query that
// resolves returns immediately, only a genuinely broken one waits longer.
configure({ asyncUtilTimeout: 5000 });
