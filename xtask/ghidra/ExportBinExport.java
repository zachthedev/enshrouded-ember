// Exports the analyzed program to a BinExport v2 file, which is what BinDiff reads.
// One script argument: the absolute output path.
// @category Ember

import com.google.security.binexport.BinExportExporter;
import ghidra.app.script.GhidraScript;
import java.io.File;

public class ExportBinExport extends GhidraScript {

    @Override
    public void run() throws Exception {
        String[] args = getScriptArgs();
        if (args.length != 1) {
            throw new IllegalArgumentException("expected one argument, the output path");
        }

        File output = new File(args[0]);
        File parent = output.getParentFile();
        if (parent != null && !parent.exists() && !parent.mkdirs()) {
            throw new java.io.IOException("cannot create " + parent);
        }

        long start = System.nanoTime();
        BinExportExporter exporter = new BinExportExporter();
        boolean exported = exporter.export(output, currentProgram, null, monitor);
        long millis = (System.nanoTime() - start) / 1_000_000L;

        if (!exported) {
            println("binexport log: " + exporter.getMessageLog().toString());
            throw new RuntimeException("BinExport refused " + currentProgram.getName());
        }

        println("binexport wrote " + output.getAbsolutePath() + " bytes=" + output.length()
            + " millis=" + millis
            + " functions=" + currentProgram.getFunctionManager().getFunctionCount());
    }
}
