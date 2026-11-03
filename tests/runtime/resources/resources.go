package resources

import (
	"context"
	"encoding/binary"
	"fmt"
	"sync"

	wrpc "wrpc.io/go"

	"driver/resources/exports/exports_"
	"driver/resources/imports"
)

var (
	mu sync.Mutex
	xs []int32
)

type handler struct {
	c wrpc.Invoker
}

func NewHandler(c wrpc.Invoker) handler {
	return handler{c}
}

func (handler) Add(ctx context.Context, a wrpc.Borrow[exports_.Z], b wrpc.Borrow[exports_.Z]) (wrpc.Own[exports_.Z], error) {
	val := int32(binary.LittleEndian.Uint32(a)) + int32(binary.LittleEndian.Uint32(b))
	return binary.LittleEndian.AppendUint32(nil, uint32(val)), nil
}

func (handler) Consume(ctx context.Context, x wrpc.Own[exports_.X]) error {
	return nil
}

func (h handler) TestImports(ctx context.Context) (*wrpc.Result[struct{}, string], error) {
	y := must(imports.NewY(ctx, h.c, 10))
	assertEqual(must(imports.Y_GetA(ctx, h.c, y.Borrow())), 10)
	must0(imports.Y_SetA(ctx, h.c, y.Borrow(), 20))
	assertEqual(must(imports.Y_GetA(ctx, h.c, y.Borrow())), 20)
	y2 := must(imports.Y_Add(ctx, h.c, y, 20))
	assertEqual(must(imports.Y_GetA(ctx, h.c, y2.Borrow())), 40)

	// test multiple instances
	y1 := must(imports.NewY(ctx, h.c, 1))
	y2 = must(imports.NewY(ctx, h.c, 2))
	assertEqual(must(imports.Y_GetA(ctx, h.c, y1.Borrow())), 1)
	assertEqual(must(imports.Y_GetA(ctx, h.c, y2.Borrow())), 2)
	must0(imports.Y_SetA(ctx, h.c, y1.Borrow(), 10))
	must0(imports.Y_SetA(ctx, h.c, y2.Borrow(), 20))
	assertEqual(must(imports.Y_GetA(ctx, h.c, y1.Borrow())), 10)
	assertEqual(must(imports.Y_GetA(ctx, h.c, y2.Borrow())), 20)
	y3 := must(imports.Y_Add(ctx, h.c, y1, 20))
	y4 := must(imports.Y_Add(ctx, h.c, y2, 30))
	assertEqual(must(imports.Y_GetA(ctx, h.c, y3.Borrow())), 30)
	assertEqual(must(imports.Y_GetA(ctx, h.c, y4.Borrow())), 50)
	return wrpc.Ok[string](struct{}{}), nil
}

func (handler) X(ctx context.Context, a int32) (wrpc.Own[exports_.X], error) {
	mu.Lock()
	defer mu.Unlock()
	idx := uint32(len(xs))
	xs = append(xs, a)
	return binary.LittleEndian.AppendUint32(nil, idx), nil
}

func (handler) X_GetA(ctx context.Context, self wrpc.Borrow[exports_.X]) (int32, error) {
	mu.Lock()
	defer mu.Unlock()
	return xs[binary.LittleEndian.Uint32(self)], nil
}

func (handler) X_SetA(ctx context.Context, self wrpc.Borrow[exports_.X], a int32) error {
	mu.Lock()
	defer mu.Unlock()
	xs[binary.LittleEndian.Uint32(self)] = a
	return nil
}

func (handler) X_Add(ctx context.Context, x wrpc.Own[exports_.X], a int32) (wrpc.Own[exports_.X], error) {
	mu.Lock()
	defer mu.Unlock()
	xs[binary.LittleEndian.Uint32(x)] += a
	return x, nil
}

func (handler) Z(ctx context.Context, a int32) (wrpc.Own[exports_.Z], error) {
	return binary.LittleEndian.AppendUint32(nil, uint32(a)), nil
}

func (handler) Z_GetA(ctx context.Context, self wrpc.Borrow[exports_.Z]) (int32, error) {
	return int32(binary.LittleEndian.Uint32(self)), nil
}

func (handler) Z_NumDropped(ctx context.Context) (uint32, error) {
	return 1, nil
}

func (handler) KebabCase(ctx context.Context, a uint32) (wrpc.Own[exports_.KebabCase], error) {
	return binary.LittleEndian.AppendUint32(nil, a), nil
}

func (handler) KebabCase_GetA(ctx context.Context, self wrpc.Borrow[exports_.KebabCase]) (uint32, error) {
	return binary.LittleEndian.Uint32(self), nil
}

func (h handler) KebabCase_TakeOwned(ctx context.Context, k wrpc.Own[exports_.KebabCase]) (uint32, error) {
	return h.KebabCase_GetA(ctx, k.Borrow())
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
