package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/flags_widths/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	f17, b := must2(to_test.RoundtripFlag17(ctx, c, &to_test.Flag17{B0: true, B16: true}, 7))
	assertEqual(*f17, to_test.Flag17{B0: true, B16: true})
	assertEqual(b, 7)

	f40, b := must2(to_test.RoundtripFlag40(ctx, c, &to_test.Flag40{B1: true, B39: true}, 9))
	assertEqual(*f40, to_test.Flag40{B1: true, B39: true})
	assertEqual(b, 9)
	return nil
}

func must2[A, B any](a A, b B, err error) (A, B) {
	if err != nil {
		panic(err)
	}
	return a, b
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
