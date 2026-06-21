module github.com/example/project

go 1.22

toolchain go1.22.3

require (
	github.com/spf13/cobra v1.8.0
	golang.org/x/sync v0.7.0 // indirect
	gopkg.in/yaml.v3 v3.0.1
)

require github.com/stretchr/testify v1.9.0

replace github.com/old/pkg => github.com/new/pkg v1.2.3

exclude github.com/bad/pkg v0.1.0

retract v1.0.1 // published by mistake
