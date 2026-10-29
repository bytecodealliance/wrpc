package wrpc_test

import (
	"bytes"
	"testing"

	wrpc "wrpc.io/go"
)

func TestWriteFutureUnit(t *testing.T) {
	ch := make(chan struct{}, 1)
	ch <- struct{}{}
	close(ch)
	var buf bytes.Buffer
	if write, err := wrpc.WriteFutureUnit(ch, &buf); err != nil || write != nil || !bytes.Equal(buf.Bytes(), []byte{1}) {
		t.Fatalf("resolved future should be written as ready, got `%v`, `%v`", buf.Bytes(), err)
	}
	if _, err := wrpc.WriteFutureUnit(nil, &buf); err == nil {
		t.Fatal("writing a nil future should fail")
	}
}
