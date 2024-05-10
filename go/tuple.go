package wrpc

import (
	"fmt"
	"log/slog"
)

type Tuple2[T0, T1 any] struct {
	V0 T0
	V1 T1
}

func ReadTuple2[T0, T1 any](r ByteReader, f0 func(ByteReader) (T0, error), f1 func(ByteReader) (T1, error)) (*Tuple2[T0, T1], error,
) {
	v0, err := f0(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 0: %w", err)
	}
	v1, err := f1(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 1: %w", err)
	}
	return &Tuple2[T0, T1]{v0, v1}, nil
}

func (v *Tuple2[T0, T1]) WriteTo(w ByteWriter, f0 func(T0, ByteWriter) error, f1 func(T1, ByteWriter) error) error {
	slog.Debug("writing tuple element 0")
	if err := f0(v.V0, w); err != nil {
		return fmt.Errorf("failed to write tuple element 0: %w", err)
	}
	slog.Debug("writing tuple element 1")
	if err := f1(v.V1, w); err != nil {
		return fmt.Errorf("failed to write tuple element 1: %w", err)
	}
	return nil
}

type Tuple3[T0, T1, T2 any] struct {
	V0 T0
	V1 T1
	V2 T2
}

func ReadTuple3[T0, T1, T2 any](r ByteReader, f0 func(ByteReader) (T0, error), f1 func(ByteReader) (T1, error), f2 func(ByteReader) (T2, error)) (*Tuple3[T0, T1, T2], error,
) {
	v0, err := f0(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 0: %w", err)
	}
	v1, err := f1(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 1: %w", err)
	}
	v2, err := f2(r)
	if err != nil {
		return nil, fmt.Errorf("failed to read tuple element 2: %w", err)
	}
	return &Tuple3[T0, T1, T2]{v0, v1, v2}, nil
}

func (v *Tuple3[T0, T1, T2]) WriteTo(w ByteWriter, f0 func(T0, ByteWriter) error, f1 func(T1, ByteWriter) error, f2 func(T2, ByteWriter) error) error {
	slog.Debug("writing tuple element 0")
	if err := f0(v.V0, w); err != nil {
		return fmt.Errorf("failed to write tuple element 0: %w", err)
	}
	slog.Debug("writing tuple element 1")
	if err := f1(v.V1, w); err != nil {
		return fmt.Errorf("failed to write tuple element 1: %w", err)
	}
	slog.Debug("writing tuple element 2")
	if err := f2(v.V2, w); err != nil {
		return fmt.Errorf("failed to write tuple element 2: %w", err)
	}
	return nil
}

type Tuple4[T0, T1, T2, T3 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
}

type Tuple5[T0, T1, T2, T3, T4 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
}

type Tuple6[T0, T1, T2, T3, T4, T5 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
}

type Tuple7[T0, T1, T2, T3, T4, T5, T6 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
	V6 T6
}

type Tuple8[T0, T1, T2, T3, T4, T5, T6, T7 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
	V6 T6
	V7 T7
}

type Tuple9[T0, T1, T2, T3, T4, T5, T6, T7, T8 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
	V6 T6
	V7 T7
	V8 T8
}

type Tuple10[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9 any] struct {
	V0 T0
	V1 T1
	V2 T2
	V3 T3
	V4 T4
	V5 T5
	V6 T6
	V7 T7
	V8 T8
	V9 T9
}

type Tuple11[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
}

type Tuple12[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
}

type Tuple13[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
	V12 T12
}

type Tuple14[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
	V12 T12
	V13 T13
}

type Tuple15[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
	V12 T12
	V13 T13
	V14 T14
}

type Tuple16[T0, T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11, T12, T13, T14, T15 any] struct {
	V0  T0
	V1  T1
	V2  T2
	V3  T3
	V4  T4
	V5  T5
	V6  T6
	V7  T7
	V8  T8
	V9  T9
	V10 T10
	V11 T11
	V12 T12
	V13 T13
	V14 T14
	V15 T15
}
