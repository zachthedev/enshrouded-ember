// Creates a function at every address in a list, so an entry nothing calls reaches the export.
// One script argument: the file holding the addresses, one hexadecimal virtual address per line.
// @category Ember

import ghidra.app.script.GhidraScript;
import ghidra.program.model.address.Address;
import ghidra.program.model.listing.Function;
import java.io.BufferedReader;
import java.io.File;
import java.io.FileReader;

public class SeedFunctions extends GhidraScript {

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 1) {
            throw new IllegalArgumentException("expected one argument, the address list");
        }

        File list = new File(args[0]);
        if (!list.isFile()) {
            throw new java.io.FileNotFoundException("no address list at " + list.getAbsolutePath());
        }

        int read = 0;
        int already = 0;
        int created = 0;
        int refused = 0;

        try (BufferedReader reader = new BufferedReader(new FileReader(list))) {
            String line;
            while ((line = reader.readLine()) != null) {
                String text = line.trim();
                if (text.isEmpty() || text.startsWith("#")) {
                    continue;
                }
                read += 1;
                Address entry = parse(text);
                if (getFunctionAt(entry) != null) {
                    already += 1;
                    continue;
                }
                // A program entry is referenced only from its record in .rdata, so
                // auto-analysis leaves the bytes as data. They become code here, and
                // a function can then start there.
                if (getInstructionAt(entry) == null) {
                    disassemble(entry);
                }
                Function made = createFunction(entry, null);
                if (made == null) {
                    refused += 1;
                    println("seed refused " + entry);
                } else {
                    created += 1;
                }
            }
        }

        println(
            "seed read=" + read + " already=" + already + " created=" + created + " refused=" + refused
                + " functions=" + currentProgram.getFunctionManager().getFunctionCount());
    }

    /** Reads one hexadecimal virtual address, with or without the 0x prefix. */
    private Address parse(String text) {
        String digits = text.startsWith("0x") || text.startsWith("0X") ? text.substring(2) : text;
        return toAddr(Long.parseUnsignedLong(digits, 16));
    }
}
