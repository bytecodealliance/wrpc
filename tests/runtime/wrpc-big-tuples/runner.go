package runner

import (
	"context"
	"fmt"

	wrpc "wrpc.io/go"

	"driver/runner/wrpc_test/big_tuples/to_test"
)

func Run(ctx context.Context, connect func() (*wrpc.Framed, error)) error {
	c, err := connect()
	if err != nil {
		return err
	}
	a := &to_test.T17{V0: 0, V1: 1, V2: 2, V3: 3, V4: 4, V5: 5, V6: 6, V7: 7, V8: 8, V9: 9, V10: 10, V11: 11, V12: 12, V13: 13, V14: 14, V15: 15, V16: "foo"}
	res, err := to_test.Roundtrip(ctx, c, []*to_test.T17{a})
	if err != nil {
		return err
	}
	if len(res) != 1 || *res[0] != *a {
		return fmt.Errorf("unexpected roundtrip result: %v", res)
	}

	fut := make(chan uint8, 1)
	fut <- 16
	close(fut)
	rx, inv, err := to_test.NestedFuture(ctx, c, &struct {
		V0  uint8
		V1  uint8
		V2  uint8
		V3  uint8
		V4  uint8
		V5  uint8
		V6  uint8
		V7  uint8
		V8  uint8
		V9  uint8
		V10 uint8
		V11 uint8
		V12 uint8
		V13 uint8
		V14 uint8
		V15 uint8
		V16 <-chan uint8
	}{V0: 0, V1: 1, V2: 2, V3: 3, V4: 4, V5: 5, V6: 6, V7: 7, V8: 8, V9: 9, V10: 10, V11: 11, V12: 12, V13: 13, V14: 14, V15: 15, V16: fut})
	if err != nil {
		return err
	}
	done := make(chan error, 1)
	go func() { done <- inv.Wait() }()
	if v := <-rx; v != 16 {
		return fmt.Errorf("unexpected future value: %d", v)
	}
	if err := <-done; err != nil {
		return err
	}

	s, err := to_test.ManyParams(ctx, c, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, "foo")
	if err != nil {
		return err
	}
	if s != "foo" {
		return fmt.Errorf("unexpected many-params result: %s", s)
	}
	return nil
}
