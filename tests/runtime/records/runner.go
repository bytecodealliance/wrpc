package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/records/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	a, b := must2(to_test.MultipleResults(ctx, c))
	assertEqual(a, 4)
	assertEqual(b, 5)

	c0, c1 := must2(to_test.SwapTuple(ctx, c, &wrpc.Tuple2[uint8, uint32]{V0: 1, V1: 2}))
	assertEqual(c0, 2)
	assertEqual(c1, 1)

	assertEqual(*must(to_test.RoundtripFlags1(ctx, c, &to_test.F1{A: true})), to_test.F1{A: true})
	assertEqual(*must(to_test.RoundtripFlags1(ctx, c, &to_test.F1{})), to_test.F1{})
	assertEqual(*must(to_test.RoundtripFlags1(ctx, c, &to_test.F1{B: true})), to_test.F1{B: true})
	assertEqual(*must(to_test.RoundtripFlags1(ctx, c, &to_test.F1{A: true, B: true})), to_test.F1{A: true, B: true})

	assertEqual(*must(to_test.RoundtripFlags2(ctx, c, &to_test.F2{C: true})), to_test.F2{C: true})
	assertEqual(*must(to_test.RoundtripFlags2(ctx, c, &to_test.F2{})), to_test.F2{})
	assertEqual(*must(to_test.RoundtripFlags2(ctx, c, &to_test.F2{D: true})), to_test.F2{D: true})
	assertEqual(*must(to_test.RoundtripFlags2(ctx, c, &to_test.F2{C: true, E: true})), to_test.F2{C: true, E: true})

	f8, f16, f32 := must3(to_test.RoundtripFlags3(ctx, c, &to_test.Flag8{B0: true}, &to_test.Flag16{B1: true}, &to_test.Flag32{B2: true}))
	assertEqual(*f8, to_test.Flag8{B0: true})
	assertEqual(*f16, to_test.Flag16{B1: true})
	assertEqual(*f32, to_test.Flag32{B2: true})

	r := must(to_test.RoundtripRecord1(ctx, c, &to_test.R1{A: 8, B: &to_test.F1{}}))
	assertEqual(r.A, 8)
	assertEqual(*r.B, to_test.F1{})

	r = must(to_test.RoundtripRecord1(ctx, c, &to_test.R1{A: 0, B: &to_test.F1{A: true, B: true}}))
	assertEqual(r.A, 0)
	assertEqual(*r.B, to_test.F1{A: true, B: true})

	assertEqual(must(to_test.Tuple1(ctx, c, 1)), 1)
	return nil
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func must2[A, B any](a A, b B, err error) (A, B) {
	if err != nil {
		panic(err)
	}
	return a, b
}

func must3[A, B, C any](a A, b B, c C, err error) (A, B, C) {
	if err != nil {
		panic(err)
	}
	return a, b, c
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
