package test

import "context"

type v1 struct{}

func (v1) X(ctx context.Context) (float32, error) {
	return 1.0, nil
}

func (v1) Y(ctx context.Context, a float32) (float32, error) {
	return 1.0 + a, nil
}

type v2 struct{}

func (v2) X(ctx context.Context) (float32, error) {
	return 2.0, nil
}

func (v2) Z(ctx context.Context, a float32, b float32) (float32, error) {
	return 2.0 + a + b, nil
}

func NewHandler() (v1, v2) {
	return v1{}, v2{}
}
