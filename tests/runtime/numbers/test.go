package test

import "context"

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) RoundtripU8(ctx context.Context, v uint8) (uint8, error) {
	return v, nil
}

func (handler) RoundtripS8(ctx context.Context, v int8) (int8, error) {
	return v, nil
}

func (handler) RoundtripU16(ctx context.Context, v uint16) (uint16, error) {
	return v, nil
}

func (handler) RoundtripS16(ctx context.Context, v int16) (int16, error) {
	return v, nil
}

func (handler) RoundtripU32(ctx context.Context, v uint32) (uint32, error) {
	return v, nil
}

func (handler) RoundtripS32(ctx context.Context, v int32) (int32, error) {
	return v, nil
}

func (handler) RoundtripU64(ctx context.Context, v uint64) (uint64, error) {
	return v, nil
}

func (handler) RoundtripS64(ctx context.Context, v int64) (int64, error) {
	return v, nil
}

func (handler) RoundtripF32(ctx context.Context, v float32) (float32, error) {
	return v, nil
}

func (handler) RoundtripF64(ctx context.Context, v float64) (float64, error) {
	return v, nil
}

func (handler) RoundtripChar(ctx context.Context, v rune) (rune, error) {
	return v, nil
}

var scalar uint32 = 0

func (handler) SetScalar(ctx context.Context, v uint32) error {
	scalar = v
	return nil
}

func (handler) GetScalar(ctx context.Context) (uint32, error) {
	return scalar, nil
}
