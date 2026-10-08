# `builtin/`

The explicit registry of modules included in a build. Static libraries need this
central reference so otherwise-unreferenced module objects are retained.
