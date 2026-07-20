package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/strings/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	must0(to_test.TakeBasic(ctx, c, "latin utf16"))
	assertEqual(must(to_test.ReturnUnicode(ctx, c)), "🚀🚀🚀 𠈄𓀀")
	assertEqual(must(to_test.ReturnEmpty(ctx, c)), "")
	assertEqual(must(to_test.Roundtrip(ctx, c, "🚀🚀🚀 𠈄𓀀")), "🚀🚀🚀 𠈄𓀀")
	return nil
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func must0(err error) {
	if err != nil {
		panic(err)
	}
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
