.PHONY: test contract contract-live cover

test:
	python3 -m unittest discover -s tests -v

# Every linearctl call in the design, against the 0.1.0 snapshot.
contract:
	python3 scripts/check_contract.py docs/design.md

# The same against the linearctl on PATH (catches drift from the snapshot).
contract-live:
	python3 scripts/check_contract.py docs/design.md --live linearctl

cover:
	uvx --from coverage coverage run --branch --include='scripts/*' -m unittest discover -s tests
	uvx --from coverage coverage report -m
