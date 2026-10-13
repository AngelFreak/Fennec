package io.github.fennec.recorder

import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.MultiFormatReader
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.common.HybridBinarizer
import io.github.fennec.recorder.net.PairingUri
import org.junit.Assert.assertEquals
import org.junit.Test
import javax.imageio.ImageIO

/**
 * Fennec's QR code as the desktop draws it (a screenshot of Settings → Phone
 * from the end-to-end run) is read by the same decoder the camera uses, and
 * parses as a pairing target.
 */
class QrTest {
    @Test
    fun `the code Fennec shows is read and understood`() {
        val img = ImageIO.read(javaClass.getResourceAsStream("/desktop-pairing.png"))
        val pixels = IntArray(img.width * img.height).also { img.getRGB(0, 0, img.width, img.height, it, 0, img.width) }
        val bitmap = BinaryBitmap(HybridBinarizer(RGBLuminanceSource(img.width, img.height, pixels)))
        val text = MultiFormatReader().decode(
            bitmap,
            mapOf(DecodeHintType.POSSIBLE_FORMATS to listOf(BarcodeFormat.QR_CODE), DecodeHintType.TRY_HARDER to true),
        ).text
        val target = PairingUri.parse(text)!!
        assertEquals("127.0.0.1:47131", target.address)
        assertEquals("96066654", target.token)
        assertEquals("runcho", target.name)
        assertEquals(43, target.pin!!.length)
    }
}
