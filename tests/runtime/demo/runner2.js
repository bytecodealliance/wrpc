import * as theTest from "./a/b/the-test.js";

export async function run(connect) {
  await theTest.x(await connect());
}
