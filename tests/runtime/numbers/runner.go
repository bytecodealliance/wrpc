package runner

import (
	"context"
	"fmt"
	"math"

	wrpc "wrpc.io/go"
	"driver/runner/test/numbers/numbers"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	assertEqual(must(numbers.RoundtripU8(ctx, c, 1)), 1)
	assertEqual(must(numbers.RoundtripU8(ctx, c, 0)), 0)
	assertEqual(must(numbers.RoundtripU8(ctx, c, math.MaxUint8)), math.MaxUint8)

	assertEqual(must(numbers.RoundtripS8(ctx, c, 1)), 1)
	assertEqual(must(numbers.RoundtripS8(ctx, c, math.MinInt8)), math.MinInt8)
	assertEqual(must(numbers.RoundtripS8(ctx, c, math.MaxInt8)), math.MaxInt8)

	assertEqual(must(numbers.RoundtripU16(ctx, c, 1)), 1)
	assertEqual(must(numbers.RoundtripU16(ctx, c, 0)), 0)
	assertEqual(must(numbers.RoundtripU16(ctx, c, math.MaxUint16)), math.MaxUint16)

	assertEqual(must(numbers.RoundtripS16(ctx, c, 1)), 1)
	assertEqual(must(numbers.RoundtripS16(ctx, c, math.MinInt16)), math.MinInt16)
	assertEqual(must(numbers.RoundtripS16(ctx, c, math.MaxInt16)), math.MaxInt16)

	assertEqual(must(numbers.RoundtripU32(ctx, c, 1)), 1)
	assertEqual(must(numbers.RoundtripU32(ctx, c, 0)), 0)
	assertEqual(must(numbers.RoundtripU32(ctx, c, math.MaxUint32)), math.MaxUint32)

	assertEqual(must(numbers.RoundtripS32(ctx, c, 1)), 1)
	assertEqual(must(numbers.RoundtripS32(ctx, c, math.MinInt32)), math.MinInt32)
	assertEqual(must(numbers.RoundtripS32(ctx, c, math.MaxInt32)), math.MaxInt32)

	assertEqual(must(numbers.RoundtripU64(ctx, c, 1)), 1)
	assertEqual(must(numbers.RoundtripU64(ctx, c, 0)), 0)
	assertEqual(must(numbers.RoundtripU64(ctx, c, math.MaxUint64)), math.MaxUint64)

	assertEqual(must(numbers.RoundtripS64(ctx, c, 1)), 1)
	assertEqual(must(numbers.RoundtripS64(ctx, c, math.MinInt64)), math.MinInt64)
	assertEqual(must(numbers.RoundtripS64(ctx, c, math.MaxInt64)), math.MaxInt64)

	assertEqual(must(numbers.RoundtripF32(ctx, c, 1.0)), 1.0)
	assertEqual(must(numbers.RoundtripF32(ctx, c, float32(math.Inf(1)))), float32(math.Inf(1)))
	assertEqual(must(numbers.RoundtripF32(ctx, c, float32(math.Inf(-1)))), float32(math.Inf(-1)))
	assert(math.IsNaN(float64(must(numbers.RoundtripF32(ctx, c, float32(math.NaN()))))))

	assertEqual(must(numbers.RoundtripF64(ctx, c, 1.0)), 1.0)
	assertEqual(must(numbers.RoundtripF64(ctx, c, math.Inf(1))), math.Inf(1))
	assertEqual(must(numbers.RoundtripF64(ctx, c, math.Inf(-1))), math.Inf(-1))
	assert(math.IsNaN(must(numbers.RoundtripF64(ctx, c, math.NaN()))))

	assertEqual(must(numbers.RoundtripChar(ctx, c, 'a')), 'a')
	assertEqual(must(numbers.RoundtripChar(ctx, c, ' ')), ' ')
	assertEqual(must(numbers.RoundtripChar(ctx, c, '🚩')), '🚩')

	must0(numbers.SetScalar(ctx, c, 2))
	assertEqual(must(numbers.GetScalar(ctx, c)), 2)

	must0(numbers.SetScalar(ctx, c, 4))
	assertEqual(must(numbers.GetScalar(ctx, c)), 4)
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

func assert(v bool) {
	if !v {
		panic("assertion failed")
	}
}
