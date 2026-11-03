import * as bar1 from "./my/inline/bar1.js";
import * as bar2 from "./my/inline/bar2.js";
import * as foo1 from "./my/inline/foo1.js";
import * as foo2 from "./my/inline/foo2.js";

export async function run(connect) {
  await foo1.foo(await connect());
  await foo2.foo(await connect());
  await bar1.bar(await connect());
  await bar2.bar(await connect());
}
