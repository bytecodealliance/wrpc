import assert from "node:assert/strict";

import * as component from "./exports/foo/bar/component.js";

assert.ok(component.UnusedEnum && component.UnusedRecord && component.UnusedVariant);

export const handlers = () => [{ foo() {} }];
