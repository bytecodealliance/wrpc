package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/test/variants/to_test"
)

func Run(ctx context.Context, c wrpc.Invoker) error {
	assertEqual(*must(to_test.RoundtripOption(ctx, c, ptr[float32](1.0))), 1)
	assertNil(must(to_test.RoundtripOption(ctx, c, nil)))
	assertEqual(*must(to_test.RoundtripOption(ctx, c, ptr[float32](2.0))), 2)

	assertEqual(*must(to_test.RoundtripResult(ctx, c, wrpc.Ok[float32, uint32](2))).Ok, 2.0)
	assertEqual(*must(to_test.RoundtripResult(ctx, c, wrpc.Ok[float32, uint32](4))).Ok, 4.0)
	assertEqual(*must(to_test.RoundtripResult(ctx, c, wrpc.Err[uint32, float32](5.3))).Err, 5)

	assertEqual(must(to_test.RoundtripEnum(ctx, c, to_test.E1_A)), to_test.E1_A)
	assertEqual(must(to_test.RoundtripEnum(ctx, c, to_test.E1_B)), to_test.E1_B)

	assertEqual(must(to_test.InvertBool(ctx, c, true)), false)
	assertEqual(must(to_test.InvertBool(ctx, c, false)), true)

	{
		a1, a2, a3, a4, a5, a6 := must6(to_test.VariantCasts(ctx, c, &to_test.Casts{
			V0: to_test.NewC1A(1),
			V1: to_test.NewC2A(2),
			V2: to_test.NewC3A(3),
			V3: to_test.NewC4A(4),
			V4: to_test.NewC5A(5),
			V5: to_test.NewC6A(6.0),
		}))
		assertVariant(a1.GetA())
		assertVariant(a2.GetA())
		assertVariant(a3.GetA())
		assertVariant(a4.GetA())
		assertVariant(a5.GetA())
		assertVariant(a6.GetA())
	}

	{
		a1, a2, a3, a4, a5, a6 := must6(to_test.VariantCasts(ctx, c, &to_test.Casts{
			V0: to_test.NewC1B(1),
			V1: to_test.NewC2B(2.0),
			V2: to_test.NewC3B(3.0),
			V3: to_test.NewC4B(4.0),
			V4: to_test.NewC5B(5.0),
			V5: to_test.NewC6B(6.0),
		}))
		assertVariant(a1.GetB())
		assertVariant(a2.GetB())
		assertVariant(a3.GetB())
		assertVariant(a4.GetB())
		assertVariant(a5.GetB())
		assertVariant(a6.GetB())
	}

	{
		a1, a2, a3, a4 := must4(to_test.VariantZeros(ctx, c, &to_test.Zeros{
			V0: to_test.NewZ1A(1),
			V1: to_test.NewZ2A(2),
			V2: to_test.NewZ3A(3.0),
			V3: to_test.NewZ4A(4.0),
		}))
		assertVariant(a1.GetA())
		assertVariant(a2.GetA())
		assertVariant(a3.GetA())
		assertVariant(a4.GetA())
	}

	{
		a1, a2, a3, a4 := must4(to_test.VariantZeros(ctx, c, &to_test.Zeros{
			V0: to_test.NewZ1B(),
			V1: to_test.NewZ2B(),
			V2: to_test.NewZ3B(),
			V3: to_test.NewZ4B(),
		}))
		assertEqual(a1.Discriminant(), to_test.Z1B)
		assertEqual(a2.Discriminant(), to_test.Z2B)
		assertEqual(a3.Discriminant(), to_test.Z3B)
		assertEqual(a4.Discriminant(), to_test.Z4B)
	}

	must0(to_test.VariantTypedefs(ctx, c, nil, false, wrpc.Err[uint32, struct{}](struct{}{})))

	{
		a, b, cc := must3(to_test.VariantEnums(ctx, c, true, wrpc.Ok[struct{}, struct{}](struct{}{}), to_test.MyErrno_Success))
		assertEqual(a, true)
		assert(b.Ok != nil)
		assertEqual(cc, to_test.MyErrno_Success)
	}
	return nil
}

func ptr[T any](v T) *T {
	return &v
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

func must3[A, B, C any](a A, b B, c C, err error) (A, B, C) {
	if err != nil {
		panic(err)
	}
	return a, b, c
}

func must4[A, B, C, D any](a A, b B, c C, d D, err error) (A, B, C, D) {
	if err != nil {
		panic(err)
	}
	return a, b, c, d
}

func must6[A, B, C, D, E, F any](a A, b B, c C, d D, e E, f F, err error) (A, B, C, D, E, F) {
	if err != nil {
		panic(err)
	}
	return a, b, c, d, e, f
}

func assertVariant[T any](_ T, ok bool) {
	if !ok {
		panic("unexpected variant discriminant")
	}
}

func assertNil[T any](v *T) {
	if v != nil {
		panic(fmt.Sprintf("expected nil, got %v", *v))
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
