package test

import (
	"context"
	"encoding/binary"

	wrpc "wrpc.io/go"

	"driver/test/exports/test/resource_aggregates/to_test"
)

type handler struct{}

func NewHandler() handler {
	return handler{}
}

func (handler) Thing(ctx context.Context, v uint32) (wrpc.Own[to_test.Thing], error) {
	return binary.LittleEndian.AppendUint32(nil, v+1), nil
}

func (handler) Foo(
	ctx context.Context,
	r1 *to_test.R1,
	r2 *to_test.R2,
	r3 *to_test.R3,
	t1 *to_test.T1,
	t2 wrpc.Borrow[to_test.Thing],
	v1 *to_test.V1,
	v2 *to_test.V2,
	l1 []wrpc.Own[to_test.Thing],
	l2 []wrpc.Borrow[to_test.Thing],
	o1 *wrpc.Own[to_test.Thing],
	o2 *wrpc.Borrow[to_test.Thing],
	result1 *wrpc.Result[wrpc.Own[to_test.Thing], struct{}],
	result2 *wrpc.Result[wrpc.Borrow[to_test.Thing], struct{}],
) (uint32, error) {
	val := binary.LittleEndian.Uint32
	var sum uint32
	sum += val(r1.Thing)
	sum += val(r2.Thing)
	sum += val(r3.Thing1)
	sum += val(r3.Thing2)
	sum += val(t1.V0)
	sum += val(t1.V1.Thing)
	sum += val(t2)
	if v, ok := v1.GetThing(); ok {
		sum += val(v)
	}
	if v, ok := v2.GetThing(); ok {
		sum += val(v)
	}
	for _, v := range l1 {
		sum += val(v)
	}
	for _, v := range l2 {
		sum += val(v)
	}
	if o1 != nil {
		sum += val(*o1)
	}
	if o2 != nil {
		sum += val(*o2)
	}
	if result1.Ok != nil {
		sum += val(*result1.Ok)
	}
	if result2.Ok != nil {
		sum += val(*result2.Ok)
	}
	return sum + 3, nil
}
