package wrpc_test

import (
	"bytes"
	"io"
	"testing"

	wrpc "wrpc.io/go"
)

func TestInvokeFramedHeaderUTF8(t *testing.T) {
	for _, tc := range []struct {
		instance, name string
		ok             bool
	}{
		{"i", "f", true},
		{"\xff", "f", false},
		{"i", "\xff", false},
	} {
		var w bytes.Buffer
		_, _, err := wrpc.InvokeFramed(t.Context(), nopWriteCloser{&w}, io.NopCloser(&bytes.Buffer{}), tc.instance, tc.name)
		if (err == nil) != tc.ok {
			t.Fatalf("unexpected result invoking %q/%q: %v", tc.instance, tc.name, err)
		}
		if !tc.ok && w.Len() > 0 {
			t.Fatalf("invoking %q/%q with invalid UTF-8 wrote %q", tc.instance, tc.name, w.Bytes())
		}
	}
}
