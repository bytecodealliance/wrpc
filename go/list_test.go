package wrpc_test

import (
	"bytes"
	"errors"
	"io"
	"math"
	"testing"

	wrpc "wrpc.io/go"
)

func TestUntrustedLength(t *testing.T) {
	want := bytes.Repeat([]byte{0x42}, 1<<20+1)
	got, err := wrpc.ReadBytes(bytes.NewReader(want), uint32(len(want)))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(got, want) {
		t.Fatal("read bytes do not match")
	}
	if _, err := wrpc.ReadBytes(bytes.NewReader(nil), math.MaxUint32); !errors.Is(err, io.ErrUnexpectedEOF) {
		t.Fatalf("expected %v, got %v", io.ErrUnexpectedEOF, err)
	}
	if v := wrpc.NewSlice[[]uint64](math.MaxUint32); len(v) != 0 || cap(v) > 1<<20 {
		t.Fatalf("unexpected slice length %d and capacity %d", len(v), cap(v))
	}
}

func TestInvalidUTF8String(t *testing.T) {
	if _, err := wrpc.WriteString("\xff", &bytes.Buffer{}); err == nil {
		t.Fatal("writing invalid UTF-8 should fail")
	}
	if _, err := wrpc.ReadString(bytes.NewReader([]byte{1, 0xff})); err == nil {
		t.Fatal("reading invalid UTF-8 should fail")
	}
}
