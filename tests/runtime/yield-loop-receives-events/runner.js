import * as iRunner from "./test/common/i-runner.js";

export async function run(connect) {
  await iRunner.f(await connect());
}
