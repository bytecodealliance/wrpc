import * as foo from "./my/inline/foo.js";

export async function run(connect) {
  await foo.Bar.new(await connect());
}
