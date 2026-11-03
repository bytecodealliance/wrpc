package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	float2 "driver/runner/exports_"
	"driver/runner/test/resource_floats/test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	a := must(test.NewFloat(ctx, c, 42.0))
	b := must(test.NewFloat(ctx, c, 55.0))
	float3 := must(Add(ctx, c, a.Borrow(), b.Borrow()))
	assertEqual(must(test.Float_Get(ctx, c, float3.Borrow())), 114.0)

	float4 := must(float2.NewFloat(ctx, c, 22.0))
	assertEqual(must(float2.Float_Get(ctx, c, float4.Borrow())), 22.+1.+2.+4.+3.)

	res := must(float2.Float_Add(ctx, c, float4, 7.0))
	assertEqual(must(float2.Float_Get(ctx, c, res.Borrow())), 59.0)
	return nil
}

func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func assertEqual[T comparable](a T, b T) {
	if a != b {
		panic(fmt.Sprintf("%v not equal to %v", a, b))
	}
}
