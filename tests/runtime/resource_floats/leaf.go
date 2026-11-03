package leaf

import (
	"context"
	"encoding/binary"
	"math"

	wrpc "wrpc.io/go"

	"driver/leaf/exports/imports"
	"driver/leaf/exports/test/resource_floats/test"
)

func myFloat(v float64) []byte {
	return binary.LittleEndian.AppendUint64(nil, math.Float64bits(v))
}

func val(repr []byte) float64 {
	return math.Float64frombits(binary.LittleEndian.Uint64(repr))
}

type testHandler struct{}

func (testHandler) Float(ctx context.Context, v float64) (wrpc.Own[test.Float], error) {
	return myFloat(v + 1.0), nil
}

func (testHandler) Float_Get(ctx context.Context, self wrpc.Borrow[test.Float]) (float64, error) {
	return val(self) + 3.0, nil
}

type importsHandler struct{}

func (importsHandler) Float(ctx context.Context, v float64) (wrpc.Own[imports.Float], error) {
	return myFloat(v + 2.0), nil
}

func (importsHandler) Float_Get(ctx context.Context, self wrpc.Borrow[imports.Float]) (float64, error) {
	return val(self) + 4.0, nil
}

func (h importsHandler) Float_Add(ctx context.Context, a wrpc.Own[imports.Float], b float64) (wrpc.Own[imports.Float], error) {
	return h.Float(ctx, val(a)+b+6.0)
}

func NewHandler() (testHandler, importsHandler) {
	return testHandler{}, importsHandler{}
}
